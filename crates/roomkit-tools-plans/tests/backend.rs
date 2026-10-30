//! Plan storage and cross-language contract checks.
use roomkit_tools_core::Error;
use roomkit_tools_plans::{
    MemoryPlanBackend, PlanBackend, PlanContract, PlanCreateInput, PlanDeleteInput, PlanListInput,
    PlanReadInput, PlanScope, PlanStep, PlanUpdateInput, StepStatus, decode,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn scope() -> PlanScope {
    PlanScope {
        room_id: "conversation".into(),
        caller_id: "assistant".into(),
    }
}
fn create() -> PlanCreateInput {
    PlanCreateInput {
        title: "Review shipping".into(),
        objective: "Check the threshold".into(),
        steps: vec![PlanStep {
            step_id: "inspect".into(),
            description: "Read the source".into(),
            status: StepStatus::Pending,
        }],
    }
}
fn update(id: &str, rev: u64) -> PlanUpdateInput {
    PlanUpdateInput {
        plan_id: id.into(),
        expected_revision: rev,
        title: Some("Reviewed".into()),
        objective: None,
        steps: None,
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "assertions on fixture results in a shared test helper"
)]
async fn assert_other_scopes_refused(
    backend: &MemoryPlanBackend,
    owner: &PlanScope,
    read: &PlanReadInput,
    token: &CancellationToken,
) {
    for other in [
        PlanScope {
            room_id: "other".into(),
            ..owner.clone()
        },
        PlanScope {
            caller_id: "other".into(),
            ..owner.clone()
        },
    ] {
        assert!(
            backend
                .list(&other, &PlanListInput {}, token)
                .await
                .unwrap()
                .plans
                .is_empty()
        );
        let missing = backend
            .read(
                &other,
                &PlanReadInput {
                    plan_id: "missing".into(),
                },
                token,
            )
            .await
            .unwrap_err();
        assert_eq!(
            backend
                .read(&other, read, token)
                .await
                .unwrap_err()
                .to_string(),
            missing.to_string()
        );
        assert!(matches!(
            backend
                .update(&other, &update(&read.plan_id, 1), token)
                .await,
            Err(Error::Refused(_))
        ));
        assert!(matches!(
            backend
                .delete(
                    &other,
                    &PlanDeleteInput {
                        plan_id: read.plan_id.clone(),
                        expected_revision: 1
                    },
                    token
                )
                .await,
            Err(Error::Refused(_))
        ));
    }
}

#[tokio::test]
async fn ownership_revisions_noop_delete_and_capacity_are_enforced() {
    let backend = MemoryPlanBackend::new(2, 1).unwrap();
    let owner = scope();
    let token = CancellationToken::new();
    let plan = backend.create(&owner, &create(), &token).await.unwrap();
    let read = PlanReadInput {
        plan_id: plan.plan_id.clone(),
    };
    assert_other_scopes_refused(&backend, &owner, &read, &token).await;
    assert!(matches!(
        backend.create(&owner, &create(), &token).await,
        Err(Error::LimitExceeded(_))
    ));
    let mut input = update(&plan.plan_id, 1);
    input.title = Some(plan.title.clone());
    assert_eq!(backend.update(&owner, &input, &token).await.unwrap(), plan);
    input.title = Some("Reviewed".into());
    let edited = backend.update(&owner, &input, &token).await.unwrap();
    assert_eq!(edited.revision, 2);
    assert_eq!(edited.objective, plan.objective);
    assert_eq!(edited.steps, plan.steps);
    assert!(matches!(
        backend.update(&owner, &input, &token).await,
        Err(Error::Refused(_))
    ));
    let mut deletion = PlanDeleteInput {
        plan_id: plan.plan_id.clone(),
        expected_revision: 1,
    };
    assert!(matches!(
        backend.delete(&owner, &deletion, &token).await,
        Err(Error::Refused(_))
    ));
    assert_eq!(backend.read(&owner, &read, &token).await.unwrap(), edited);
    deletion.expected_revision = 2;
    assert!(
        backend
            .delete(&owner, &deletion, &token)
            .await
            .unwrap()
            .deleted
    );
    assert!(backend.delete(&owner, &deletion, &token).await.is_err());
    let new = backend.create(&owner, &create(), &token).await.unwrap();
    assert_ne!(new.plan_id, plan.plan_id);
    // Separate stores are not a global registry; explicit clones share ownership.
    assert_eq!(
        backend
            .clone()
            .list(&owner, &PlanListInput {}, &token)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
    assert!(
        MemoryPlanBackend::default()
            .list(&owner, &PlanListInput {}, &token)
            .await
            .unwrap()
            .plans
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_updates_have_one_winner_without_lost_changes() {
    let backend = MemoryPlanBackend::default();
    let owner = scope();
    let token = CancellationToken::new();
    let plan = backend.create(&owner, &create(), &token).await.unwrap();
    let gate = Arc::new(tokio::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for title in ["First", "Second"] {
        let backend = backend.clone();
        let owner = owner.clone();
        let token = token.clone();
        let gate = Arc::clone(&gate);
        let mut input = update(&plan.plan_id, 1);
        input.title = Some(title.into());
        workers.push(tokio::spawn(async move {
            gate.wait().await;
            backend.update(&owner, &input, &token).await
        }));
    }
    gate.wait().await;
    let mut winners = 0;
    for worker in workers {
        match worker.await.unwrap() {
            Ok(value) => {
                winners += 1;
                assert_eq!(value.revision, 2);
            }
            Err(Error::Refused(message)) => assert!(message.contains("revision conflict")),
            other => panic!("unexpected result {other:?}"),
        }
    }
    assert_eq!(winners, 1);
    assert_eq!(
        backend
            .read(
                &owner,
                &PlanReadInput {
                    plan_id: plan.plan_id
                },
                &token
            )
            .await
            .unwrap()
            .revision,
        2
    );
}

#[tokio::test]
async fn validation_size_and_precancellation_do_not_mutate_state() {
    let backend = MemoryPlanBackend::default();
    let owner = scope();
    let token = CancellationToken::new();
    let mut input = create();
    input.steps.push(input.steps[0].clone());
    assert!(matches!(
        backend.create(&owner, &input, &token).await,
        Err(Error::InvalidInput(_))
    ));
    let mut input = create();
    input.steps[0].description = "x".repeat(2049);
    assert!(matches!(input.validate(), Err(Error::LimitExceeded(_))));
    assert!(
        decode::<PlanCreateInput>(r#"{"title":"T","objective":"O","steps":[],"room_id":"spoof"}"#)
            .is_err()
    );
    assert!(
        decode::<PlanUpdateInput>(r#"{"plan_id":"p","expected_revision":0,"title":"T"}"#).is_err()
    );
    assert!(decode::<PlanUpdateInput>(r#"{"plan_id":"p","expected_revision":1}"#).is_err());
    assert!(decode::<PlanUpdateInput>(r#"{"plan_id":"p","title":"T"}"#).is_err());
    let mut input = create();
    input.objective = "o".repeat(16 * 1024);
    let plan = backend.create(&owner, &input, &token).await.unwrap();
    // A valid partial input can exceed the record limit when merged with existing fields.
    let mut change = update(&plan.plan_id, 1);
    change.steps = Some(
        (0..30)
            .map(|i| PlanStep {
                step_id: format!("s{i}"),
                description: "s".repeat(2048),
                status: StepStatus::InProgress,
            })
            .collect(),
    );
    change.validate().unwrap();
    assert!(matches!(
        backend.update(&owner, &change, &token).await,
        Err(Error::LimitExceeded(_))
    ));
    let stopped = CancellationToken::new();
    stopped.cancel();
    assert!(matches!(
        backend.create(&owner, &create(), &stopped).await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        backend
            .update(&owner, &update(&plan.plan_id, 1), &stopped)
            .await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        backend
            .delete(
                &owner,
                &PlanDeleteInput {
                    plan_id: plan.plan_id.clone(),
                    expected_revision: 1
                },
                &stopped
            )
            .await,
        Err(Error::Cancelled)
    ));
    assert_eq!(
        backend
            .read(
                &owner,
                &PlanReadInput {
                    plan_id: plan.plan_id
                },
                &token
            )
            .await
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        backend
            .list(&owner, &PlanListInput {}, &token)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
}

#[tokio::test]
async fn bounded_listing_can_recover_all_escaped_titles() {
    let backend = MemoryPlanBackend::new(64, 64).unwrap();
    let mut input = create();
    input.title = format!("x{}", "\u{1}".repeat(255));
    for _ in 0..64 {
        backend
            .create(&scope(), &input, &CancellationToken::new())
            .await
            .unwrap();
    }
    let result = backend
        .list(&scope(), &PlanListInput {}, &CancellationToken::new())
        .await
        .unwrap();
    let output = roomkit_tools_plans::render(result).unwrap();
    assert_eq!(output.data.plans.len(), 64);
    decode::<roomkit_tools_plans::PlanListResult>(&serde_json::to_string(&output.data).unwrap())
        .unwrap();
}
