//! Topic bindings: which catalogue stream each `Topic` is carried on.
//!
//! See ADR 0100 §1 for the event fabric's architecture and §5 for the
//! catalogue's quality-of-service classes.
//!
//! Fixed as `(topic, schema_version, class)` constants for the six reflex
//! topics only — `ReflexPassMarked`, `ReflexJournalRecorded`,
//! `MarketEventApplied`, `ReflexOutcomeRecorded`, `ReflexChainSpan` and
//! `EventFabricGap` — because those are the bodies ADR 0100 §1's placement
//! table names as the fabric's own P1/P2 traffic, and the only ones this
//! build fixes a class for. `qip-contracts::reflex` owns the reflex journal
//! contract's actual payload types; this module never names one, and
//! `qip-contracts` is never made to name `Topic` in return — the binding is
//! a constant table `qip-events` holds about its own closed set, not a
//! dependency edge in either direction.

use crate::topic::Topic;

use super::policy::QosClass;

/// One reflex topic's fixed binding: the schema version this build writes,
/// and the catalogue class the fabric carries it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TopicBinding {
    pub topic: Topic,
    pub schema_version: u32,
    pub qos_class: QosClass,
}

/// ADR 0100 §5's P2 row: "every reflex journal entry."
pub const REFLEX_PASS_MARKED: TopicBinding = TopicBinding {
    topic: Topic::ReflexPassMarked,
    schema_version: 1,
    qos_class: QosClass::P2MarketJournal,
};

/// ADR 0100 §5's P2 row: "every reflex journal entry."
pub const REFLEX_JOURNAL_RECORDED: TopicBinding = TopicBinding {
    topic: Topic::ReflexJournalRecorded,
    schema_version: 1,
    qos_class: QosClass::P2MarketJournal,
};

/// `Topic::MarketEventApplied`'s own doc comment: "on P2 alongside the
/// journal entries it produced (ADR 0100 §5)."
pub const MARKET_EVENT_APPLIED: TopicBinding = TopicBinding {
    topic: Topic::MarketEventApplied,
    schema_version: 1,
    qos_class: QosClass::P2MarketJournal,
};

/// ADR 0100 §5's P1 row: "fills, cancels, settlement records."
pub const REFLEX_OUTCOME_RECORDED: TopicBinding = TopicBinding {
    topic: Topic::ReflexOutcomeRecorded,
    schema_version: 1,
    qos_class: QosClass::P1Outcomes,
};

/// ADR 0100 §5's P1 row: "`ChainSpan` continuity records."
pub const REFLEX_CHAIN_SPAN: TopicBinding = TopicBinding {
    topic: Topic::ReflexChainSpan,
    schema_version: 1,
    qos_class: QosClass::P1Outcomes,
};

/// `Topic::EventFabricGap`'s own doc comment: never dropped, because only
/// the producer that declared the gap knows where it fell.
pub const EVENT_FABRIC_GAP: TopicBinding = TopicBinding {
    topic: Topic::EventFabricGap,
    schema_version: 1,
    qos_class: QosClass::P1Outcomes,
};

/// Every reflex binding, in declaration order.
pub const ALL: [TopicBinding; 6] = [
    REFLEX_PASS_MARKED,
    REFLEX_JOURNAL_RECORDED,
    MARKET_EVENT_APPLIED,
    REFLEX_OUTCOME_RECORDED,
    REFLEX_CHAIN_SPAN,
    EVENT_FABRIC_GAP,
];

/// The binding for `topic`, or `None` for every topic outside the six the
/// reflex fabric writes.
pub fn binding_for(topic: Topic) -> Option<TopicBinding> {
    ALL.iter().copied().find(|binding| binding.topic == topic)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every one of the six reflex topics is bound, on the class ADR 0100
    /// §5 names for it, and nothing outside the six is.
    ///
    /// Asserts its own premise first — that there are exactly six bindings,
    /// not "at least one found" against a table that could have been left
    /// empty.
    #[test]
    fn every_reflex_topic_has_exactly_one_binding_on_the_class_adr_0100_names() {
        assert_eq!(ALL.len(), 6);
        let expectations = [
            (Topic::ReflexPassMarked, QosClass::P2MarketJournal),
            (Topic::ReflexJournalRecorded, QosClass::P2MarketJournal),
            (Topic::MarketEventApplied, QosClass::P2MarketJournal),
            (Topic::ReflexOutcomeRecorded, QosClass::P1Outcomes),
            (Topic::ReflexChainSpan, QosClass::P1Outcomes),
            (Topic::EventFabricGap, QosClass::P1Outcomes),
        ];
        for (topic, class) in expectations {
            let binding = binding_for(topic).unwrap_or_else(|| panic!("{topic} has no binding"));
            assert_eq!(binding.qos_class, class, "{topic} bound to the wrong class");
        }
        assert!(binding_for(Topic::MarketTick).is_none());
    }
}
