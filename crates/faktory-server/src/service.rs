//! Tonic implementation of every Faktory RPC.

use std::pin::Pin;

use faktory_proto::v1::{
    DeleteViewRequest, DeleteViewResponse, GetModelRequest, GetModelResponse, ListModelsRequest,
    ListModelsResponse, ListViewsRequest, ListViewsResponse, ModelChanged, ModelSnapshot,
    PutViewRequest, PutViewResponse, SetDefaultViewRequest, SetDefaultViewResponse,
    WatchModelsRequest, WatchModelsResponse, faktory_service_server::FaktoryService,
    watch_models_response::Event,
};
use futures_util::Stream;
use tokio_stream::{StreamExt as _, once, wrappers::BroadcastStream};
use tonic::{Request, Response, Status};

use crate::model::{Repository, RepositoryError};

#[derive(Clone, Debug)]
pub struct FaktoryGrpcService {
    repository: Repository,
}

impl FaktoryGrpcService {
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self { repository }
    }
}

type WatchStream = Pin<Box<dyn Stream<Item = Result<WatchModelsResponse, Status>> + Send>>;

#[tonic::async_trait]
impl FaktoryService for FaktoryGrpcService {
    type WatchModelsStream = WatchStream;

    async fn list_models(
        &self,
        _: Request<ListModelsRequest>,
    ) -> Result<Response<ListModelsResponse>, Status> {
        let models = self
            .repository
            .list_models()
            .await
            .map_err(status)?
            .iter()
            .map(crate::model::ModelRecord::to_proto)
            .collect();
        Ok(Response::new(ListModelsResponse { models }))
    }

    async fn get_model(
        &self,
        request: Request<GetModelRequest>,
    ) -> Result<Response<GetModelResponse>, Status> {
        let model = self
            .repository
            .get_model(&request.into_inner().model_id)
            .await
            .map_err(status)?
            .record
            .to_proto();
        Ok(Response::new(GetModelResponse { model: Some(model) }))
    }

    async fn list_views(
        &self,
        request: Request<ListViewsRequest>,
    ) -> Result<Response<ListViewsResponse>, Status> {
        let model_id = request.into_inner().model_id;
        let model = self
            .repository
            .get_model(&model_id)
            .await
            .map_err(status)?
            .record;
        let views = self
            .repository
            .list_views(&model_id)
            .await
            .map_err(status)?
            .iter()
            .map(crate::model::ViewRecord::to_proto)
            .collect();
        Ok(Response::new(ListViewsResponse {
            views,
            default_view_id: model.default_view_id,
        }))
    }

    async fn put_view(
        &self,
        request: Request<PutViewRequest>,
    ) -> Result<Response<PutViewResponse>, Status> {
        let request = request.into_inner();
        let view = request
            .view
            .ok_or_else(|| Status::invalid_argument("view is required"))?;
        let view = self
            .repository
            .put_view(&request.model_id, view, request.expected_etag.as_deref())
            .await
            .map_err(status)?
            .to_proto();
        Ok(Response::new(PutViewResponse { view: Some(view) }))
    }

    async fn delete_view(
        &self,
        request: Request<DeleteViewRequest>,
    ) -> Result<Response<DeleteViewResponse>, Status> {
        let request = request.into_inner();
        self.repository
            .delete_view(&request.model_id, &request.view_id, &request.expected_etag)
            .await
            .map_err(status)?;
        Ok(Response::new(DeleteViewResponse {}))
    }

    async fn set_default_view(
        &self,
        request: Request<SetDefaultViewRequest>,
    ) -> Result<Response<SetDefaultViewResponse>, Status> {
        let request = request.into_inner();
        let model = self
            .repository
            .set_default_view(&request.model_id, &request.view_id)
            .await
            .map_err(status)?
            .to_proto();
        Ok(Response::new(SetDefaultViewResponse { model: Some(model) }))
    }

    async fn watch_models(
        &self,
        _: Request<WatchModelsRequest>,
    ) -> Result<Response<Self::WatchModelsStream>, Status> {
        let receiver = self.repository.subscribe();
        let models = self
            .repository
            .list_models()
            .await
            .map_err(status)?
            .iter()
            .map(crate::model::ModelRecord::to_proto)
            .collect();
        let initial = Ok(WatchModelsResponse {
            event: Some(Event::InitialSnapshot(ModelSnapshot { models })),
        });
        let changes = BroadcastStream::new(receiver).map(|result| match result {
            Ok(change) => Ok(WatchModelsResponse {
                event: Some(Event::ModelChanged(ModelChanged {
                    model: Some(change.0.to_proto()),
                })),
            }),
            Err(_) => Err(Status::resource_exhausted("watch stream lagged")),
        });
        Ok(Response::new(Box::pin(once(initial).chain(changes))))
    }
}

#[must_use]
pub fn status(error: RepositoryError) -> Status {
    match error {
        RepositoryError::Invalid => Status::invalid_argument("invalid argument"),
        RepositoryError::NotFound => Status::not_found("not found"),
        RepositoryError::Conflict => Status::aborted("concurrent mutation conflict"),
        RepositoryError::Unavailable => Status::unavailable("service unavailable"),
        RepositoryError::Corrupt => Status::internal("stored state is invalid"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;
    use faktory_proto::v1::RenderState;

    use super::*;
    use crate::{
        model::{
            GeometryFactsRecord, GeometrySizeRecord, ModelOutputSummaryRecord, OutputManifest,
            OutputRoleRecord, RenderedModelOutput, RenderedOutput, TechnicalProjectionImages,
        },
        storage::InMemoryObjectStore,
    };

    fn rendered() -> RenderedOutput {
        let summary = ModelOutputSummaryRecord {
            output_id: "primary".to_owned(),
            role: OutputRoleRecord::Assembly,
            primary: true,
            facts: GeometryFactsRecord {
                volume_cubic_millimeters: 24.0,
                size_millimeters: GeometrySizeRecord {
                    x: 2.0,
                    y: 3.0,
                    z: 4.0,
                },
            },
        };
        RenderedOutput {
            manifest: OutputManifest {
                format: OutputManifest::FORMAT.to_owned(),
                outputs: vec![summary.clone()],
            }
            .canonical_bytes()
            .expect("manifest"),
            outputs: vec![RenderedModelOutput {
                summary,
                glb: Bytes::from_static(b"glb"),
                preview: Bytes::from_static(b"<svg></svg>"),
                projections: TechnicalProjectionImages::all(Bytes::from_static(b"png")),
                shaded: Some(TechnicalProjectionImages::all(Bytes::from_static(
                    b"shaded-png",
                ))),
            }],
        }
    }

    fn assert_complete_model(
        model: &faktory_proto::v1::Model,
        expected_name: &str,
        expected_revision: &str,
        expected_updated_at: prost_types::Timestamp,
    ) {
        assert_eq!(model.id, "part");
        assert_eq!(model.name, expected_name);
        assert_eq!(model.desired_source_revision, expected_revision);
        assert_eq!(model.current_successful_source_revision, expected_revision);
        assert_eq!(model.render_state, RenderState::Ready as i32);
        assert!(model.render_error.is_empty());
        assert!(model.default_view_id.is_empty());
        let facts = model
            .current_successful_facts
            .as_ref()
            .expect("geometry facts");
        assert!((facts.volume_cubic_millimeters - 24.0).abs() < f64::EPSILON);
        assert_eq!(
            facts.size_millimeters,
            Some(faktory_proto::v1::Vector3 {
                x: 2.0,
                y: 3.0,
                z: 4.0,
            })
        );
        assert_eq!(model.updated_at.as_ref(), Some(&expected_updated_at));
    }

    #[tokio::test]
    async fn list_get_and_watch_return_complete_model_fields() {
        let repository = Repository::new(Arc::new(InMemoryObjectStore::default()), 8);
        let created = repository
            .create_model("part", "Part", b"source")
            .await
            .expect("create model");
        repository
            .complete_render(&created.id, &created.desired_source_revision, rendered())
            .await
            .expect("complete render");
        let initial_updated_at = prost_types::Timestamp {
            seconds: created.updated_at.seconds,
            nanos: created.updated_at.nanos,
        };
        let service = FaktoryGrpcService::new(repository.clone());

        let listed = service
            .list_models(Request::new(ListModelsRequest {}))
            .await
            .expect("list models")
            .into_inner();
        assert_complete_model(
            &listed.models[0],
            "Part",
            &created.desired_source_revision,
            initial_updated_at,
        );

        let fetched = service
            .get_model(Request::new(GetModelRequest {
                model_id: created.id.clone(),
            }))
            .await
            .expect("get model")
            .into_inner()
            .model
            .expect("model");
        assert_complete_model(
            &fetched,
            "Part",
            &created.desired_source_revision,
            initial_updated_at,
        );

        let mut watched = service
            .watch_models(Request::new(WatchModelsRequest {}))
            .await
            .expect("watch models")
            .into_inner();
        let initial = watched.next().await.expect("initial event").expect("event");
        let Some(Event::InitialSnapshot(snapshot)) = initial.event else {
            panic!("expected initial snapshot");
        };
        assert_complete_model(
            &snapshot.models[0],
            "Part",
            &created.desired_source_revision,
            initial_updated_at,
        );

        let renamed = repository
            .edit_model(
                &created.id,
                &created.desired_source_revision,
                Some("Renamed"),
                None,
            )
            .await
            .expect("rename model")
            .record;
        let renamed_updated_at = prost_types::Timestamp {
            seconds: renamed.updated_at.seconds,
            nanos: renamed.updated_at.nanos,
        };
        assert_ne!(renamed_updated_at, initial_updated_at);
        let changed = watched.next().await.expect("changed event").expect("event");
        let Some(Event::ModelChanged(change)) = changed.event else {
            panic!("expected model change");
        };
        assert_complete_model(
            change.model.as_ref().expect("changed model"),
            "Renamed",
            &created.desired_source_revision,
            renamed_updated_at,
        );
    }
}
