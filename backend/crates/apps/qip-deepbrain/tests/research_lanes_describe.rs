//! The lane a promoted question was given (ADR 0149 d6, R12-EXPAND-6052) was
//! readable only from the in-memory round; the cycle line printed packages by
//! outcome and nothing said where each question went.

use qip_deepbrain::research::{PromotionLane, ResearchRound};

#[test]
fn the_round_description_names_each_promoted_questions_lane() {
    let mut round = ResearchRound::default();
    round.lanes.insert(
        "q-inside".to_string(),
        PromotionLane::Reflex {
            package_id: "pkg-inside".to_string(),
        },
    );
    round.lanes.insert(
        "q-past".to_string(),
        PromotionLane::Research {
            refusal: "past-the-bounds".to_string(),
        },
    );
    assert_eq!(round.lanes.len(), 2, "premise: two lanes recorded");
    let line = round.describe();
    assert!(
        line.contains("promotion lanes {"),
        "lanes section missing: {line}"
    );
    assert!(
        line.contains(r#""q-inside": Reflex { package_id: "pkg-inside" }"#),
        "{line}"
    );
    assert!(
        line.contains(r#""q-past": Research { refusal: "past-the-bounds" }"#),
        "{line}"
    );
}
