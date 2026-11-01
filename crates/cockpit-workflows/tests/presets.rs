//! Preset selection, versions and the capability check before dispatch.

use cockpit_provider_api::{Capabilities, Capability, Provenance};
use cockpit_tasks::Kind;
use cockpit_workflows::{
    CODE_CHANGE, DESIGN_LED, Doer, PRESETS, RESEARCH, Refusal, check_capabilities, preset,
    preset_version, presets, select_preset,
};

#[test]
fn the_product_ships_research_code_change_and_design_led() {
    let names: Vec<_> = presets()
        .iter()
        .map(|preset| (preset.name, preset.version))
        .collect();
    assert_eq!(
        names,
        [("research", 1), ("code-change", 1), ("design-led", 1)]
    );
}

#[test]
fn each_preset_has_the_steps_report_06_names() {
    for (preset, steps) in [
        (&RESEARCH, ["investigate", "synthesize", "deliver"]),
        (&CODE_CHANGE, ["implement", "check", "deliver"]),
        (&DESIGN_LED, ["design-approval", "implement", "verify"]),
    ] {
        let names: Vec<_> = preset.steps.iter().map(|step| step.name).collect();
        assert_eq!(names, steps, "{}", preset.name);
    }
}

#[test]
fn design_led_starts_with_the_owners_approval() {
    assert_eq!(DESIGN_LED.steps[0].doer, Doer::Owner);
    assert!(DESIGN_LED.steps[0].required_capabilities.is_empty());
}

#[test]
fn every_agent_step_requires_events_and_outcome() {
    for preset in PRESETS {
        for step in preset.steps.iter().filter(|step| step.doer == Doer::Agent) {
            assert_eq!(
                step.required_capabilities,
                [Capability::Events, Capability::Outcome],
                "{} {}",
                preset.name,
                step.name
            );
        }
    }
}

#[test]
fn a_preset_version_is_found_exactly() {
    assert_eq!(preset_version("code-change", 1), Some(&CODE_CHANGE));
    assert_eq!(preset_version("code-change", 2), None);
    assert_eq!(preset("design-led"), Some(&DESIGN_LED));
}

#[test]
fn the_named_preset_wins_over_the_item_kind() {
    assert_eq!(
        select_preset(Some("design-led"), Kind::Research),
        Ok(&DESIGN_LED)
    );
}

#[test]
fn without_a_name_a_research_item_follows_research() {
    assert_eq!(select_preset(None, Kind::Research), Ok(&RESEARCH));
}

#[test]
fn without_a_name_any_other_item_follows_code_change() {
    for kind in Kind::ALL.into_iter().filter(|kind| *kind != Kind::Research) {
        assert_eq!(select_preset(None, kind), Ok(&CODE_CHANGE), "{kind:?}");
    }
}

#[test]
fn an_unknown_preset_is_refused_naming_the_known_ones() {
    assert_eq!(
        select_preset(Some("solo"), Kind::Task),
        Err(Refusal::UnknownPreset {
            name: "solo".to_owned(),
            known: vec!["research", "code-change", "design-led"],
        })
    );
}

#[test]
fn a_provider_missing_a_declared_capability_is_refused_naming_it() {
    let events_only = Capabilities::none().with(Capability::Events, Provenance::Discovered);
    assert_eq!(
        check_capabilities(&CODE_CHANGE.steps[0], &events_only),
        Err(Refusal::MissingCapabilities {
            step: "implement",
            missing: vec![Capability::Outcome],
        })
    );
}

#[test]
fn discovered_capabilities_satisfy_a_step() {
    let discovered = Capabilities::none()
        .with(Capability::Events, Provenance::Discovered)
        .with(Capability::Outcome, Provenance::Discovered);
    assert_eq!(check_capabilities(&RESEARCH.steps[0], &discovered), Ok(()));
}

#[test]
fn an_owner_step_needs_no_capability() {
    assert_eq!(
        check_capabilities(&DESIGN_LED.steps[0], &Capabilities::none()),
        Ok(())
    );
}
