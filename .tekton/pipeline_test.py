import json
import re
import unittest
from pathlib import Path


ROOT = Path(__file__).parent


def pipeline(environment):
    text = (ROOT / f"faktory-{environment}.yaml").read_text()
    graph = text.split("    tasks:\n", 1)[1].split("\n  workspaces:", 1)[0]
    parsed = []
    for match in re.finditer(r"^      - name: ([\w-]+)\n([\s\S]*?)(?=^      - name: |\Z)", graph, re.M):
        name, body = match.groups()
        task = {"name": name}
        if "        matrix:\n" in body:
            task["matrix"] = {}
        if "        taskSpec:\n" in body:
            task["taskSpec"] = {}
        reference = re.search(r"^        taskRef:\n          name: ([\w-]+)$", body, re.M)
        if reference:
            task["taskRef"] = {"name": reference[1]}
            after = body.split("        runAfter:\n", 1)[1].split("        retries:", 1)[0]
            task["runAfter"] = re.findall(r"^          - ([\w-]+)$", after, re.M)
            task["params"] = [{"name": key, "value": value.strip("'")}
                              for key, value in re.findall(
                                  r"^          - name: ([\w-]+)\n            value: (.+)$", body, re.M)]
            bindings = re.findall(r"^          - name: ([\w-]+)\n            workspace: ([\w-]+)$", body, re.M)
            if bindings:
                task["workspaces"] = [{"name": key, "workspace": value} for key, value in bindings]
        elif name == "scan-private-material":
            image = re.search(r"^              image: (.+)$", body, re.M)[1]
            task["taskSpec"] = {"steps": [{"image": image}]}
        parsed.append(task)
    overrides = [{"pipelineTaskName": name} for name in re.findall(r"pipelineTaskName: ([\w-]+)", text)]
    workspace_text = text.rsplit("\n  workspaces:", 1)[1]
    workspaces = [{"name": name} for name in re.findall(r"^    - name: ([\w-]+)$", workspace_text, re.M)]
    if "    - name: git-credentials\n      secret:\n" in workspace_text:
        next(workspace for workspace in workspaces if workspace["name"] == "git-credentials")["secret"] = {
            "secretName": re.search(r'        secretName: "(.+)"', workspace_text)[1]}
    return {"spec": {"pipelineSpec": {"tasks": parsed}, "taskRunSpecs": overrides, "workspaces": workspaces}}


def tasks(environment):
    return {task["name"]: task for task in pipeline(environment)["spec"]["pipelineSpec"]["tasks"]}


def params(task):
    return {param["name"]: param["value"] for param in task["params"]}


class VisualRendererPipelineTest(unittest.TestCase):
    def setUp(self):
        self.preview = tasks("preview")
        self.release = tasks("release")
        self.verifier = (ROOT / "verify-visual-renderer.mjs").read_text()

    def test_worker_build_is_amd64_only(self):
        builds = [task for task in self.preview.values()
                  if task.get("taskRef", {}).get("name") == "service-build-v1"
                  and params(task)["target"] == "visual-renderer-runtime"]
        self.assertEqual(len(builds), 1)
        self.assertEqual(params(builds[0])["arch"], "amd64")
        self.assertEqual(params(builds[0])["expected-user"], "65532:65532")
        self.assertNotIn("git-credentials", [w["name"] for w in builds[0]["workspaces"]])

    def test_worker_uses_an_explicit_single_platform_index(self):
        manifest = self.preview["manifest-visual-renderer"]
        self.assertEqual(manifest["taskRef"]["name"], "service-manifest-v1")
        self.assertEqual(json.loads(params(manifest)["sources"]), [
            {"arch": "amd64", "image": "$(tasks.build-visual-renderer-amd64.results.image)"}])
        self.assertEqual(manifest["runAfter"], ["build-visual-renderer-amd64"])

    def test_functional_verification_is_not_in_deployment_graph(self):
        for environment in ("preview", "release"):
            document = pipeline(environment)
            graph = tasks(environment)
            self.assertFalse(any(name.startswith("verify-") for name in graph))
            for override in document["spec"].get("taskRunSpecs", []):
                self.assertIn(override["pipelineTaskName"], graph)
            text = (ROOT / f"faktory-{environment}.yaml").read_text()
            self.assertNotIn("verify-visual-renderer.mjs", text)
            self.assertNotIn("verify-companion.mjs", text)
        self.assertNotIn("--no-" + "sandbox", self.verifier)

    def test_native_verifier_renders_colored_canonical_response(self):
        self.assertIn('recipe: "three-v2"', self.verifier)
        self.assertIn('baseColorFactor: [1, 0.02, 0.02, 1]', self.verifier)
        self.assertIn('const names = ["isometric", "front", "back", "left", "right", "top", "bottom"]', self.verifier)
        self.assertIn('image.mime_type !== "image/png"', self.verifier)
        self.assertIn("png.width !== 640 || png.height !== 480", self.verifier)
        self.assertIn("redPixels < 100", self.verifier)

    def test_release_declares_immutable_worker_identity(self):
        promotion = self.release["promote-release"]
        worker = next(entry for entry in json.loads(params(promotion)["images"])
                      if entry["key"] == "visualRendererImage")
        self.assertEqual(worker, {"key": "visualRendererImage",
                                 "repository": "$(params.visual-renderer-base-image)",
                                 "platforms": ["amd64"], "user": "65532:65532"})


class CompanionDigestBindingTest(unittest.TestCase):
    def test_preview_threads_both_build_digests_to_manifest(self):
        graph = tasks("preview")
        manifest = graph["manifest-companion"]
        self.assertEqual(manifest["taskRef"]["name"], "service-manifest-v1")
        self.assertEqual(json.loads(params(manifest)["sources"]), [
            {"arch": arch, "image": f"$(tasks.build-companion-{arch}.results.image)"}
            for arch in ("amd64", "arm64")])
        self.assertEqual(manifest["runAfter"], ["build-companion-amd64", "build-companion-arm64"])
        image_config = json.loads(params(graph["deploy-preview"])["image-config"])
        self.assertEqual(image_config["companionImage"], "$(tasks.manifest-companion.results.image)")

    def test_companion_builds_are_native_and_credential_free(self):
        graph = tasks("preview")
        for arch in ("amd64", "arm64"):
            build = graph[f"build-companion-{arch}"]
            self.assertEqual(build["taskRef"]["name"], "service-build-v1")
            self.assertEqual(build["runAfter"], ["scan-private-material"])
            values = params(build)
            self.assertEqual(values["target"], "companion-runtime")
            self.assertEqual(values["arch"], arch)
            self.assertEqual(values["builder-address"], "tcp://{{ BUILDKIT_" + arch.upper() + "_ADDR }}")
            self.assertEqual(values["revision"], "$(params.revision)")
            self.assertEqual(values["expected-user"], "65532:65532")
            self.assertEqual(build["workspaces"], [{"name": "source", "workspace": "source"}])

    def test_release_promotes_shared_image_set_without_retesting(self):
        graph = tasks("release")
        self.assertNotIn("select-companion-release", graph)
        self.assertNotIn("verify-companion-release", graph)
        promotion = graph["promote-release"]
        self.assertEqual(promotion["taskRef"]["name"], "service-promote-v1")
        self.assertEqual(promotion["runAfter"], ["validate-release", "scan-private-material"])
        companion = next(entry for entry in json.loads(params(promotion)["images"])
                         if entry["key"] == "companionImage")
        self.assertEqual(companion["platforms"], ["amd64", "arm64"])
        self.assertEqual(companion["user"], "65532:65532")
        self.assertEqual(params(graph["deploy-production"])["image-config"], "$(tasks.promote-release.results.images)")


class ReleasePromotionTest(unittest.TestCase):
    def test_companion_manifest_gates_deployment(self):
        graph = tasks("preview")
        self.assertIn("manifest-companion", graph["deploy-preview"]["runAfter"])
        self.assertNotIn("build-arg:VITE_COMPANION_ORIGIN", (ROOT / "faktory-preview.yaml").read_text())

    def test_companion_packaging_is_static_and_credential_free(self):
        dockerfile = (ROOT.parent / "Dockerfile").read_text()
        runtime = dockerfile.split("AS companion-runtime\n", 1)[1]
        self.assertIn("USER 65532:65532", runtime)
        self.assertIn('"/opt/companion-host/dist/server.js"', runtime)
        self.assertNotRegex(runtime, r"FAKTORY_S3|FAKTORY_DATABASE|OIDC|--from=rust-build|--from=python-build")
        compose = (ROOT.parent / "compose.yaml").read_text()
        host = compose.split("  companion-host:\n", 1)[1].split("  faktory:\n", 1)[0]
        self.assertIn("COMPANION_PARENT_ORIGINS: http://172.16.1.40:8080", host)
        self.assertIn('"0.0.0.0:8082:8082"', host)
        self.assertIn("read_only: true", host)
        self.assertNotRegex(host, r"FAKTORY_S3|FAKTORY_DATABASE|OIDC|secret|volumes:")

    def test_release_graph_preserves_gates_without_building(self):
        text = (ROOT / "faktory-release.yaml").read_text()
        self.assertIn("secretName: faktory-release-trusted-signers", text)
        for gate in ("git verify-tag", "git cat-file -t", "git merge-base --is-ancestor",
                     "tag does not resolve to the webhook revision", "cargo_version", "web_version",
                     "gitleaks dir --config .gitleaks.toml"):
            self.assertIn(gate, text)
        for removed in ("buildctl", "BUILDKIT", "service-build-v1", "service-manifest-v1", ":sha-", ":preview-"):
            self.assertNotIn(removed, text)
        self.assertNotIn(":latest", text.replace("cr.holdenitdown.net/rfhold/general-ci:latest", ""))
        self.assertEqual(set(tasks("release")), {"fetch-repository", "validate-release", "scan-private-material",
                                                "promote-release", "deploy-production"})

    def test_promotion_is_one_shared_preflight_for_all_destinations(self):
        graph = tasks("release")
        promotions = [task for task in graph.values() if task.get("taskRef", {}).get("name") == "service-promote-v1"]
        self.assertEqual(len(promotions), 1)
        promotion = promotions[0]
        self.assertNotIn("taskSpec", promotion)
        values = params(promotion)
        self.assertEqual(set(values), {"revision", "version", "images"})
        self.assertEqual(values["revision"], "$(params.revision)")
        self.assertEqual(values["version"], "$(tasks.validate-release.results.tag)")
        self.assertEqual([entry["key"] for entry in json.loads(values["images"])],
                         ["image", "visualRendererImage", "companionImage"])

    def test_security_scan_pin_is_preserved(self):
        for environment in ("preview", "release"):
            scan = tasks(environment)["scan-private-material"]["taskSpec"]["steps"][0]
            self.assertEqual(scan["image"], "docker.io/zricethezav/gitleaks:v8.28.0@sha256:cdbb7c955abce02001a9f6c9f602fb195b7fadc1e812065883f695d1eeaba854")

    def test_pulumi_consumes_only_complete_image_maps(self):
        for environment, name, stack in (("preview", "deploy-preview", "preview"), ("release", "deploy-production", "prod")):
            deploy = tasks(environment)[name]
            self.assertEqual(deploy["taskRef"]["name"], "service-pulumi-up-v1")
            values = params(deploy)
            self.assertEqual(values["source-subdir"], "infra/pulumi")
            self.assertEqual(values["stack"], stack)
            self.assertEqual(values["authentik-enabled"], "true")
            self.assertEqual(values["grafana-enabled"], "false")
            self.assertEqual(values["openbao-enabled"], "false")
            self.assertEqual(deploy["workspaces"], [{"name": "source", "workspace": "source"}])
        graph = tasks("preview")
        images = json.loads(params(graph["deploy-preview"])["image-config"])
        self.assertEqual(images, {"image": "$(tasks.manifest-server.results.image)",
                                  "visualRendererImage": "$(tasks.manifest-visual-renderer.results.image)",
                                  "companionImage": "$(tasks.manifest-companion.results.image)"})

    def test_all_five_builds_are_independently_eligible(self):
        builds = [task for task in tasks("preview").values()
                  if task.get("taskRef", {}).get("name") == "service-build-v1"]
        self.assertEqual(len(builds), 5)
        for build in builds:
            self.assertEqual(build["runAfter"], ["scan-private-material"])
            self.assertNotIn("matrix", build)

    def test_only_private_rust_builds_receive_git_credentials(self):
        preview = pipeline("preview")["spec"]
        binding = next(w for w in preview["workspaces"] if w["name"] == "git-credentials")
        self.assertEqual(binding["secret"]["secretName"], "{{ git_auth_secret }}")
        for name, task in tasks("preview").items():
            if task.get("taskRef", {}).get("name") == "service-build-v1":
                credentials = any(w["name"] == "git-credentials" for w in task["workspaces"])
                self.assertEqual(credentials, name.startswith("build-server-"))


if __name__ == "__main__":
    unittest.main()
