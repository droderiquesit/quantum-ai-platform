//! R12-EXPAND-6032: a `TrainingCampaign` exists only for a drift that
//! justifies it, and names the data and lineage it retrained from.

use qip_core::Timestamp;
use qip_training::campaign::{CampaignDraft, TrainingCampaign};

fn draft() -> CampaignDraft {
    CampaignDraft {
        subject: "XNYS:ACME".to_string(),
        replaces: "bar-linear-XNYS:ACME@0.1.0".to_string(),
        drift_score: 0.31,
        drift_threshold: 0.2,
        worst_feature: "momentum_5".to_string(),
        dataset_digest: "ab".repeat(32),
        teacher_lineage: vec![
            "model:bar-linear-XNYS:ACME@0.1.0".to_string(),
            "fitted-on:cd".to_string(),
        ],
        successor: "bar-linear-XNYS:ACME@0.2.0".to_string(),
        opened_at: Timestamp::from_secs(1_760_000_000),
    }
}

#[test]
fn a_campaign_names_what_it_replaced_what_it_was_fitted_on_and_why() {
    let campaign = TrainingCampaign::open(draft()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(campaign.replaces(), "bar-linear-XNYS:ACME@0.1.0");
    assert_eq!(campaign.successor(), "bar-linear-XNYS:ACME@0.2.0");
    assert_eq!(campaign.dataset_digest(), "ab".repeat(32));
    assert_eq!(campaign.teacher_lineage().len(), 2);
    assert!(campaign.drift_score() > campaign.drift_threshold());
    assert!(campaign.id().starts_with("training-"), "{}", campaign.id());
    let again = TrainingCampaign::open(draft()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(campaign.id(), again.id(), "the id is derived, not drawn");
}

#[test]
fn a_model_within_its_drift_bound_opens_no_campaign() {
    // Premise: the good draft opens, so the refusals below are the
    // threshold's and not a malformed draft's.
    assert!(TrainingCampaign::open(draft()).is_ok());
    for score in [0.2, 0.05] {
        let refused = TrainingCampaign::open(CampaignDraft {
            drift_score: score,
            ..draft()
        })
        .expect_err("a drift within its threshold opened a campaign");
        assert!(
            refused.message().contains("has not passed the threshold"),
            "{}",
            refused.message()
        );
    }
}

#[test]
fn a_campaign_that_cannot_name_its_data_or_lineage_is_refused() {
    assert!(TrainingCampaign::open(draft()).is_ok());
    let bad_digest = TrainingCampaign::open(CampaignDraft {
        dataset_digest: "not-a-digest".to_string(),
        ..draft()
    })
    .expect_err("a malformed digest was admitted");
    assert!(bad_digest.message().contains("SHA-256"));
    let no_lineage = TrainingCampaign::open(CampaignDraft {
        teacher_lineage: Vec::new(),
        ..draft()
    })
    .expect_err("an empty lineage was admitted");
    assert!(no_lineage.message().contains("teacher lineage"));
    let same = TrainingCampaign::open(CampaignDraft {
        successor: draft().replaces,
        ..draft()
    })
    .expect_err("a model was its own successor");
    assert!(same.message().contains("the model it replaces"));
}
