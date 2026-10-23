mod common;
use common::sandbox::*;
use serde_json::{Value, json};
use std::fs;
use cockpit::sessions::Session;
use cockpit::subscription_fallback::{Binding, Direction, Observation, Phase, RoutingStore, Selection};
use cockpit::usage::SubscriptionCapacity::{self, Available, Exhausted, Unknown};

fn selection(agent: &str) -> Selection {
    Selection { agent: agent.into(), provider: if agent == "claude" { "anthropic" } else { "openai" }.into(),
        model: if agent == "claude" { "claude-opus-4-7" } else { "gpt-6.1-sol" }.into(),
        variant: Some("high".into()), account: "a".repeat(64) }
}

fn binding(agent: &str) -> Binding {
    let native = "aaaaaaaa-0000-4000-8000-000000000000";
    Binding { session: if agent == "claude" { SESSION_A.into() } else { cockpit::codex::cockpit_id(native).unwrap() },
        runtime: format!("runtime-{agent}"), selection: selection(agent), native_thread: (agent == "codex").then(|| native.into()) }
}

fn observation(binding: &Binding, capacity: SubscriptionCapacity) -> Observation {
    let alternate = if binding.selection.agent == "claude" { "codex" } else { "claude" };
    Observation { binding: binding.clone(), session: Session { session: binding.session.clone(),
        agent: binding.selection.agent.clone(), state: "waiting".into(), cwd: "/projects/demo".into(),
        tmux_pane: "%1".into(), ..Session::default() }, current_capacity: capacity, original_capacity: capacity,
        alternate: Some((selection(alternate), Available)), has_pending_work: true, prompt_clear: true, manual_override: false }
}

#[test]
fn exhausted_native_subscriptions_choose_the_other_subscription_in_both_directions() {
    for agent in ["claude", "codex"] {
        let sandbox = Sandbox::new();
        let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
        let source = binding(agent);
        store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
        let transitions = store.reconcile(&[observation(&source, Exhausted)], 101).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].direction, Direction::Fallback);
        assert_ne!(transitions[0].target.provider, source.selection.provider);
        assert!(store.reconcile(&[observation(&source, Exhausted)], 102).unwrap().is_empty());
    }
}

#[test]
fn unconfirmed_or_exhausted_alternatives_leave_the_original_owner_pending() {
    for capacity in [Unknown, Exhausted] {
        let sandbox = Sandbox::new();
        let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
        let source = binding("claude");
        store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
        let mut observed = observation(&source, Exhausted);
        observed.alternate.as_mut().unwrap().1 = capacity;
        assert!(store.reconcile(&[observed], 101).unwrap().is_empty());
        assert_eq!(store.route(&source.session).unwrap().current, source);
    }
}

#[test]
fn uncertain_launches_survive_restarts_without_being_submitted_again() {
    let sandbox = Sandbox::new();
    let path = sandbox.state.join("subscriptions/routes.json");
    let source = binding("claude");
    let mut store = RoutingStore::load(&path).unwrap();
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let transition = store.reconcile(&[observation(&source, Exhausted)], 101).unwrap().remove(0);
    store.mark_retired(&source.session, &transition.generation).unwrap();
    store.mark_submitted(&source.session, &transition.generation).unwrap();
    drop(store);
    let mut restored = RoutingStore::load(&path).unwrap();
    assert!(restored.reconcile(&[observation(&source, Exhausted)], 102).unwrap().is_empty());
    let Phase::Pending(pending) = &restored.route(&source.session).unwrap().phase else { panic!("lost pending launch") };
    assert_eq!(pending.generation, transition.generation);
}

#[test]
fn fresh_recovery_restores_the_original_model_and_variant_after_a_verified_replacement() {
    let sandbox = Sandbox::new();
    let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    let source = binding("claude");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let transition = store.reconcile(&[observation(&source, Exhausted)], 101).unwrap().remove(0);
    store.mark_retired(&source.session, &transition.generation).unwrap();
    store.mark_submitted(&source.session, &transition.generation).unwrap();
    let replacement = binding("codex");
    store.record_runtime(&transition.generation, replacement.clone(), "/projects/demo").unwrap();
    store.finish(&source.session, &transition.generation).unwrap();
    let mut observed = observation(&replacement, Available);
    observed.original_capacity = Unknown;
    assert!(store.reconcile(&[observed.clone()], 102).unwrap().is_empty());
    observed.original_capacity = Available;
    let returned = store.reconcile(&[observed], 103).unwrap();
    assert_eq!(returned.len(), 1);
    assert_eq!(returned[0].direction, Direction::Return);
    assert_eq!(returned[0].target, source.selection);
}

#[test]
fn working_background_and_attention_boundaries_never_prepare_a_handoff() {
    for boundary in ["working", "approval", "question", "background", "agents", "typed", "terminal", "credits", "restart"] {
        let sandbox = Sandbox::new();
        let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
        let source = binding("claude");
        store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
        let mut observed = observation(&source, Exhausted);
        match boundary {
            "working" => observed.session.state = "working".into(),
            "approval" => observed.session.ask_to_run = "approval".into(),
            "question" => observed.session.needs_input = true,
            "background" => observed.session.background = 1,
            "agents" => observed.session.agents = 1,
            "typed" => observed.prompt_clear = false,
            "terminal" => observed.session.terminal = true,
            "credits" => observed.session.credits = true,
            "restart" => observed.session.restarting = true,
            _ => unreachable!(),
        }
        assert!(store.reconcile(&[observed], 101).unwrap().is_empty(), "{boundary}");
    }
}

#[test]
fn completion_or_manual_override_cancels_automatic_work() {
    for completed in [true, false] {
        let sandbox = Sandbox::new();
        let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
        let source = binding("claude");
        store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
        let mut observed = observation(&source, Exhausted);
        if completed { observed.has_pending_work = false; } else { observed.manual_override = true; }
        assert!(store.reconcile(&[observed], 101).unwrap().is_empty());
        assert!(matches!(store.route(&source.session).unwrap().phase, Phase::Completed | Phase::Cancelled));
    }
}

#[test]
fn another_dispatcher_cannot_take_a_held_store_lock() {
    let sandbox = Sandbox::new();
    let path = sandbox.state.join("subscriptions/routes.json");
    let store = RoutingStore::load(&path).unwrap();
    assert!(RoutingStore::load(&path).is_err());
    drop(store);
    assert!(RoutingStore::load(&path).is_ok());
}

#[test]
fn corrupt_routing_state_is_preserved_and_refused() {
    let sandbox = Sandbox::new();
    let path = sandbox.state.join("routes.json");
    fs::write(&path, "not routing state").unwrap();
    assert!(RoutingStore::load(&path).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "not routing state");
}

#[test]
fn the_replacement_cannot_be_enrolled_as_a_second_logical_task() {
    let sandbox = Sandbox::new();
    let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    let source = binding("claude");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let transition = store.reconcile(&[observation(&source, Exhausted)], 101).unwrap().remove(0);
    store.mark_retired(&source.session, &transition.generation).unwrap();
    store.mark_submitted(&source.session, &transition.generation).unwrap();
    let replacement = binding("codex");
    store.record_runtime(&transition.generation, replacement.clone(), "/projects/demo").unwrap();
    store.track(replacement.clone(), "/projects/demo", "1797cdd6", 102).unwrap();
    assert!(store.route(&replacement.session).is_none());
    store.finish(&source.session, &transition.generation).unwrap();
    store.track(replacement.clone(), "/projects/demo", "1797cdd6", 103).unwrap();
    assert!(store.route(&replacement.session).is_none());
}

#[test]
fn native_fallback_does_not_silently_replace_a_cli_with_opencode() {
    let sandbox = Sandbox::new();
    let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    let source = binding("codex");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let mut observed = observation(&source, Exhausted);
    let mut alternate = selection("claude");
    alternate.agent = "opencode".into();
    observed.alternate = Some((alternate, Available));
    assert!(store.reconcile(&[observed], 101).unwrap().is_empty());
}

#[test]
fn repeated_verified_receipts_are_idempotent_but_other_checkouts_are_rejected() {
    let sandbox = Sandbox::new();
    let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    let source = binding("claude");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let transition = store.reconcile(&[observation(&source, Exhausted)], 101).unwrap().remove(0);
    let replacement = binding("codex");
    assert!(store.record_runtime(&transition.generation, replacement.clone(), "/projects/demo").is_err());
    store.mark_retired(&source.session, &transition.generation).unwrap();
    store.mark_submitted(&source.session, &transition.generation).unwrap();
    assert!(store.record_runtime(&transition.generation, replacement.clone(), "/projects/another").is_err());
    assert!(store.record_runtime("obsolete-generation", replacement.clone(), "/projects/demo").is_err());
    store.record_runtime(&transition.generation, replacement.clone(), "/projects/demo").unwrap();
    store.record_runtime(&transition.generation, replacement, "/projects/demo").unwrap();
}

#[test]
fn a_forked_child_cannot_keep_the_dispatcher_lock_after_its_owner_releases_it() {
    let sandbox = Sandbox::new();
    let path = sandbox.state.join("subscriptions/routes.json");
    let store = RoutingStore::load(&path).unwrap();
    let mut pipe = [0; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        unsafe {
            libc::close(pipe[1]);
            let mut byte = 0u8;
            libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
            libc::_exit(0);
        }
    }
    unsafe { libc::close(pipe[0]); }
    drop(store);
    let restored = RoutingStore::load(&path);
    unsafe {
        libc::write(pipe[1], b"x".as_ptr().cast(), 1);
        libc::close(pipe[1]);
        libc::waitpid(child, std::ptr::null_mut(), 0);
    }
    assert!(restored.is_ok(), "the unrelated child retained the routing lock");
}

fn claude_history(records: &[Value]) -> Sandbox {
    let sandbox = Sandbox::new();
    let directory = sandbox.home.join(".claude/projects/demo");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(format!("{SESSION_A}.jsonl")), records.iter().map(|record| format!("{record}\n")).collect::<String>()).unwrap();
    fs::write(sandbox.runtime.join("cockpit.json"), json!({"sessions":[{
        "session":SESSION_A, "agent":"claude", "state":"waiting", "cwd":"/projects/demo", "task":"Finish the requested change"
    }]}).to_string()).unwrap();
    sandbox
}

fn message(role: &str, text: &str) -> Value {
    json!({"type":role, "message":{"content":[{"type":"text", "text":text}]}})
}

fn context(sandbox: &Sandbox) -> std::process::Output {
    sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["context", SESSION_A], &[])
}

#[test]
fn handoff_context_preserves_the_latest_user_request_outside_the_visible_tail() {
    let mut records = vec![message("user", "Finish only this task; do not push")];
    records.extend((0..110).map(|index| message("assistant", &format!("Progress {index}"))));
    let sandbox = claude_history(&records);
    let output = context(&sandbox);
    assert!(output.status.success(), "{}", stderr(&output));
    let handoff: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(handoff["latest_user_request"], "Finish only this task; do not push");
    assert_eq!(handoff["history"].as_array().unwrap().len(), 100);
    assert_eq!(handoff["truncated"], true);
    assert!(handoff["instructions"].as_str().unwrap().contains("not authorization"));
    assert!(handoff["instructions"].as_str().unwrap().contains("current files"));
}

#[test]
fn handoff_context_does_not_include_reasoning_or_tool_results() {
    let sandbox = claude_history(&[
        message("user", "Finish the tests"),
        json!({"type":"assistant", "message":{"content":[{"type":"thinking", "thinking":"private reasoning"},
            {"type":"text", "text":"I changed the test"}]}}),
        json!({"type":"user", "message":{"content":[{"type":"tool_result", "content":"private tool output"}]}})
    ]);
    let output = context(&sandbox);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private"));
    let handoff: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(handoff["latest_user_request"], "Finish the tests");
}

#[test]
fn unavailable_latest_user_request_stops_the_handoff_instead_of_guessing_a_task() {
    let sandbox = claude_history(&[message("assistant", "There was some work")]);
    let output = context(&sandbox);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("latest user request"));
}
