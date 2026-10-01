use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mcp::{
    McpPrincipalId, McpTokenValidation,
    server::ServerHandler as _,
    skills::{McpSkillList, McpSkillResources, verify_skill_bytes},
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tower::ServiceExt as _;

use super::super::*;
use crate::{render::RenderConfig, storage::InMemoryObjectStore};

fn server() -> FaktoryMcp {
    let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 1);
    let renders = RenderQueue::start(
        repository.clone(),
        RenderConfig {
            command: vec!["unused".to_owned()],
            queue_capacity: 1,
            concurrency: 1,
            timeout: Duration::from_secs(1),
            max_output_bytes: 1024,
        },
    )
    .unwrap();
    FaktoryMcp::new(repository, renders).unwrap()
}

fn request(method: &str, mut params: Value) -> Request<Body> {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{"extensions":{"io.modelcontextprotocol/skills":{}}},
        "io.modelcontextprotocol/clientInfo":{"name":"faktory-test","version":"1"}
    });
    let mut builder = Request::builder();
    if method == "resources/read" {
        builder = builder.header("mcp-name", params["uri"].as_str().unwrap());
    }
    builder
        .method("POST")
        .uri("/mcp")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", method)
        .body(Body::from(
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
        ))
        .unwrap()
}

async fn rpc(router: &Router, method: &str, params: Value) -> Value {
    let response = router
        .clone()
        .oneshot(request(method, params))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    serde_json::from_str(
        text.strip_prefix("data: ")
            .and_then(|s| s.strip_suffix("\n\n"))
            .unwrap_or(text),
    )
    .unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn http_catalog_is_complete_and_every_raw_file_matches_manifest() {
    let handler = server();
    let capabilities = handler.capabilities();
    assert_eq!(
        capabilities.extensions.as_ref().unwrap()["io.modelcontextprotocol/skills"],
        json!({})
    );
    assert!(capabilities.resources.is_some());
    assert!(
        capabilities.extensions.as_ref().unwrap()["io.modelcontextprotocol/skills"]
            .get("directoryRead")
            .is_none()
    );
    let router = handler.router();
    let discovery = rpc(&router, "server/discover", json!({})).await;
    assert_eq!(
        discovery["result"]["capabilities"]["extensions"]["io.modelcontextprotocol/skills"],
        json!({})
    );
    assert_eq!(
        discovery["result"]["capabilities"]["resources"],
        json!({"subscribe":false,"listChanged":false})
    );
    let response = rpc(&router, "skills/list", json!({})).await;
    let list: McpSkillList = serde_json::from_value(response["result"].clone()).unwrap();
    assert_eq!(list.skills.len(), 3);
    assert_eq!(
        list.skills
            .iter()
            .map(|skill| skill.frontmatter["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "edit-model-projects",
            "manage-model-views",
            "publish-model-releases"
        ]
    );
    assert_eq!(response["result"]["resultType"], "complete");
    assert_eq!(response["result"]["cacheScope"], "private");
    assert_eq!(response["result"]["ttlMs"], 0);
    assert!(list.next_cursor.is_none());
    let expected_files = [
        ("edit-model-projects", "tools.md"),
        ("manage-model-views", "views.md"),
        ("publish-model-releases", "releases.md"),
    ];
    let mut count = 0;
    for (skill, (name, reference)) in list.skills.iter().zip(expected_files) {
        assert_eq!(skill.uri, format!("skill://{name}/SKILL.md"));
        let get = rpc(&router, "skills/get", json!({"uri":skill.uri})).await;
        assert_eq!(get["result"]["skill"], serde_json::to_value(skill).unwrap());
        assert_eq!(get["result"]["resultType"], "complete");
        assert_eq!(get["result"]["ttlMs"], 0);
        assert_eq!(get["result"]["cacheScope"], "private");
        let McpSkillResources::Files(files) = &skill.resources else {
            panic!("static complete manifest")
        };
        assert_eq!(files.len(), 2);
        assert_eq!(
            files
                .iter()
                .map(|file| file.uri.clone())
                .collect::<Vec<_>>(),
            [
                skill.uri.clone(),
                format!("skill://{name}/references/{reference}")
            ]
        );
        for file in files {
            let read = rpc(&router, "resources/read", json!({"uri":file.uri})).await;
            assert_eq!(read["result"]["resultType"], "complete");
            assert_eq!(read["result"]["ttlMs"], 0);
            assert_eq!(read["result"]["cacheScope"], "private");
            let text = read["result"]["contents"][0]["text"].as_str().unwrap();
            verify_skill_bytes(skill, &file.uri, text.as_bytes()).unwrap();
            assert_eq!(file.size, text.len() as u64);
            if file.uri == skill.uri {
                assert_eq!(
                    mcp::skills::parse_skill_frontmatter(text.as_bytes()).unwrap(),
                    skill.frontmatter
                );
            }
            assert!(!text.contains("storage_etag"));
            count += 1;
        }
    }
    assert_eq!(count, 6);
    for uri in [
        "skill://unknown/SKILL.md",
        "skill://edit-model-projects/../SKILL.md",
        "file:///etc/passwd",
    ] {
        assert_eq!(
            rpc(&router, "skills/get", json!({"uri":uri})).await["error"]["code"],
            -32602
        );
        assert_eq!(
            rpc(&router, "resources/read", json!({"uri":uri})).await["error"]["code"],
            -32602
        );
    }
    assert_eq!(
        rpc(&router, "skills/list", json!({"cursor":"bad"})).await["error"]["code"],
        -32602
    );
}

#[tokio::test]
async fn http_tools_preserve_uniform_names_and_schema_snapshot() {
    let router = server().router();
    let response = rpc(&router, "tools/list", json!({})).await;
    let tools = response["result"]["tools"].as_array().unwrap();
    let mut names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["create", "destroy", "edit", "execute", "query"]);
    let mut schemas = tools
        .iter()
        .map(|tool| json!({"name":tool["name"],"inputSchema":tool["inputSchema"]}))
        .collect::<Vec<_>>();
    schemas.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let digest =
        crate::model::project::hex_digest(Sha256::digest(serde_json::to_vec(&schemas).unwrap()));
    assert_eq!(
        digest,
        "fc5f93c3e913ca54a736c578f426ceea595b4295c8eb948347852be380f551d5"
    );
}

#[tokio::test]
async fn skill_http_preserves_bearer_scope_and_origin_gates() {
    let metadata =
        McpProtectedResourceMetadata::new("https://faktory.example/mcp", ["https://auth.example"])
            .with_scopes(["faktory:use"]);
    let auth = StreamableHttpAuthorization::new(metadata, |token, context| {
        Box::pin(async move {
            assert_eq!(context.required_scopes, ["faktory:use"]);
            match token.0.as_str() {
                "valid" => McpTokenValidation::Authorized {
                    principal_id: McpPrincipalId::new("test").unwrap(),
                    expires_at: None,
                },
                "wrong-scope" => McpTokenValidation::InsufficientScope {
                    required_scopes: vec!["faktory:use".to_owned()],
                    error_description: None,
                },
                _ => McpTokenValidation::Unauthorized {
                    error_description: None,
                },
            }
        })
    })
    .unwrap()
    .with_required_scopes(["faktory:use"]);
    let router = streamable_http_router_with_options(
        Arc::new(server()),
        StreamableHttpOptions::default()
            .without_root_protected_resource_metadata()
            .with_authorization(auth),
    );
    for (method, uri) in [
        ("skills/list", "skill://edit-model-projects/SKILL.md"),
        ("skills/get", "skill://edit-model-projects/SKILL.md"),
        ("resources/read", "skill://edit-model-projects/SKILL.md"),
        ("resources/read", "faktory://models"),
        ("resources/list", "faktory://models"),
        ("resources/templates/list", "faktory://models"),
    ] {
        for (token, status) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some("bad"), StatusCode::UNAUTHORIZED),
            (Some("wrong-scope"), StatusCode::FORBIDDEN),
            (Some("valid"), StatusCode::OK),
        ] {
            let mut req = request(method, json!({"uri":uri}));
            if let Some(token) = token {
                req.headers_mut()
                    .insert("authorization", format!("Bearer {token}").parse().unwrap());
            }
            let response = router.clone().oneshot(req).await.unwrap();
            assert_eq!(response.status(), status, "{method}: {token:?}");
        }
        let mut req = request(method, json!({"uri":uri}));
        req.headers_mut()
            .insert("authorization", "Bearer valid".parse().unwrap());
        req.headers_mut()
            .insert("origin", "https://untrusted.example".parse().unwrap());
        assert_eq!(
            router.clone().oneshot(req).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
}
