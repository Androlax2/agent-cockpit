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
fn a_blocked_switch_names_why_it_waits() {
    use cockpit::subscription_fallback::waiting_reason;
    let source = binding("claude");
    let mut observed = observation(&source, Exhausted);
    assert_eq!(waiting_reason(&observed, false), None, "a switch that can happen now is not waiting");
    observed.prompt_clear = false;
    assert_eq!(waiting_reason(&observed, false).as_deref(), Some("waiting for the session to finish its turn and clear any typed draft"));
    observed.prompt_clear = true;
    observed.alternate = None;
    assert_eq!(waiting_reason(&observed, false).as_deref(), Some("the other subscription's account or quota could not be read"));
    observed.alternate = Some((selection("codex"), Exhausted));
    assert_eq!(waiting_reason(&observed, false).as_deref(), Some("both subscriptions are exhausted"));
    observed.alternate = Some((selection("codex"), Unknown));
    assert_eq!(waiting_reason(&observed, false).as_deref(), Some("Codex quota is unconfirmed"));
    let mut from_codex = observation(&binding("codex"), Exhausted);
    from_codex.alternate = Some((selection("claude"), Unknown));
    assert_eq!(waiting_reason(&from_codex, false).as_deref(), Some("Claude quota is unconfirmed or its paid extra usage is on"));
    let mut healthy = observation(&source, Available);
    healthy.prompt_clear = false;
    assert_eq!(waiting_reason(&healthy, false), None, "an unexhausted session with a draft is not waiting for anything");
    healthy.original_capacity = Available;
    assert_eq!(waiting_reason(&healthy, true).as_deref(), Some("waiting for the session to finish its turn before returning to the original model"));
}

#[test]
fn waiting_reasons_persist_and_report_only_real_changes() {
    let sandbox = Sandbox::new();
    let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    let source = binding("claude");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    assert!(store.set_waiting(&source.session, Some("both subscriptions are exhausted")).unwrap());
    assert!(!store.set_waiting(&source.session, Some("both subscriptions are exhausted")).unwrap());
    drop(store);
    let routes = cockpit::subscription_fallback::read_routes(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    assert_eq!(routes[0].waiting.as_deref(), Some("both subscriptions are exhausted"));
    let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    assert!(store.set_waiting(&source.session, None).unwrap());
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
fn cancelling_a_pending_native_route_prevents_late_retirement_and_admission() {
    let sandbox = Sandbox::new();
    let mut store = RoutingStore::load(&sandbox.root.join("routes.json")).unwrap();
    let source = binding("claude");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let transition = store.reconcile(&[observation(&source, Exhausted)], 101).unwrap().remove(0);
    store.cancel(&source.session).unwrap();
    assert!(store.mark_retired(&source.session, &transition.generation).is_err());
    assert!(!store.execution_allowed(&transition.generation, &source.session, &source.runtime));
}

#[test]
fn manual_cancellation_and_completed_tasks_stop_pending_native_launches() {
    for command in ["cancel-session", "sweep"] {
        let (sandbox, _) = prepared_claude_route();
        sandbox.shim("cockpit", "exit 0");
        let arguments = if command == "sweep" { vec![command] } else { vec![command, SESSION_A] };
        let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &arguments, &[]);
        assert!(output.status.success(), "{}", stderr(&output));
        let routes = cockpit::subscription_fallback::read_routes(&sandbox.state.join("cockpit/subscriptions/routes.json")).unwrap();
        assert!(matches!(routes[0].phase, Phase::Cancelled | Phase::Completed));
        assert!(sandbox.calls("cockpit").is_empty());
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
fn prepared_handoffs_keep_source_task_metadata_after_the_source_runtime_exits() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("routes.json");
    let mut store = RoutingStore::load(&path).unwrap();
    let source = binding("claude");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let mut observed = observation(&source, Exhausted);
    observed.session.repository = "owner/repo".into();
    observed.session.branch = "todo-1797cdd6".into();
    store.reconcile(&[observed], 101).unwrap();
    drop(store);
    let store = RoutingStore::load(&path).unwrap();
    let Phase::Pending(pending) = &store.route(SESSION_A).unwrap().phase else { panic!("lost pending handoff") };
    assert_eq!(pending.source_repository, "owner/repo");
    assert_eq!(pending.source_branch, "todo-1797cdd6");
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
    assert_eq!(store.prompt_destination(&source.session), replacement.session);
    store.mark_retired(&source.session, &returned[0].generation).unwrap();
    store.mark_submitted(&source.session, &returned[0].generation).unwrap();
    let mut restored = source.clone();
    restored.runtime = "runtime-restored-original".into();
    store.record_runtime(&returned[0].generation, restored, "/projects/demo").unwrap();
    store.finish(&source.session, &returned[0].generation).unwrap();
    assert_eq!(store.prompt_destination(&replacement.session), source.session);
    assert_eq!(store.route(&source.session).unwrap().retired_sessions, [replacement.session.clone()]);
    store.track(replacement.clone(), "/projects/demo", "1797cdd6", 104).unwrap();
    assert!(store.route(&replacement.session).is_none(), "a retired task owner must not become a second logical route");
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
fn native_input_checks_reject_drafts_even_when_the_cursor_is_at_home() {
    use cockpit::subscription_fallback::native_input_is_clear;
    for (agent, line, cursor, clear) in [
        ("codex", "\x1b[1m›\x1b[0m \x1b[2mAsk Codex to do anything\x1b[0m", 2, true),
        ("codex", "\x1b[1m›\x1b[0m \x1b[2mAsk Codex to do anything\x1b[0m", 27, false),
        ("codex", "\x1b[1m›\x1b[0m Unsubmitted fixture draft", 2, false),
        ("codex", "\x1b[1m›\x1b[0m Ask Codex to do anything", 2, false),
        ("codex", "\x1b[1m›\x1b[0m \x1b[2mSelect a model\x1b[0m", 2, false),
        ("codex", "", 2, false),
        ("codex", "Quota exhausted; resets in an hour", 2, false),
        ("claude", "❯ ", 2, true),
        ("claude", "❯ Do not continue yet", 2, false),
        ("claude", "❯ ", 7, false),
        // Claude Code 2.1.309 draws its empty prompt with a no-break space (live capture 2026-10-24).
        ("claude", "❯\u{a0}", 2, true),
        ("claude", "❯\u{a0}Do not continue yet", 2, false),
        ("claude", "❯\u{a0}", 7, false),
        ("opencode", "❯ ", 2, false),
    ] {
        assert_eq!(native_input_is_clear(agent, line, cursor), clear, "{agent}/{line:?}/{cursor}");
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
fn automatic_action_readers_can_inspect_committed_routes_without_taking_the_writer_lock() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("routes.json");
    let mut writer = RoutingStore::load(&path).unwrap();
    writer.track(binding("claude"), "/projects/demo", "1797cdd6", 100).unwrap();
    let routes = cockpit::subscription_fallback::read_routes(&path).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].current.session, SESSION_A);
    assert!(RoutingStore::load(&path).is_err(), "readers must not release another dispatcher's lock");
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
fn replacement_execution_waits_for_ownership_binding_and_the_matching_runtime() {
    let sandbox = Sandbox::new();
    let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    let source = binding("claude");
    let replacement = binding("codex");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let transition = store.reconcile(&[observation(&source, Exhausted)], 101).unwrap().remove(0);
    let allowed = |store: &RoutingStore, runtime: &str| store.execution_allowed(&transition.generation, &replacement.session, runtime);
    assert!(!allowed(&store, &replacement.runtime));
    store.mark_retired(&source.session, &transition.generation).unwrap();
    store.mark_submitted(&source.session, &transition.generation).unwrap();
    store.record_runtime(&transition.generation, replacement.clone(), "/projects/demo").unwrap();
    assert!(!allowed(&store, &replacement.runtime), "a receipt is not completed ownership transfer");
    store.finish(&source.session, &transition.generation).unwrap();
    assert!(allowed(&store, &replacement.runtime));
    assert!(!allowed(&store, "impostor-runtime"));
    assert!(!store.execution_allowed(&"f".repeat(32), &replacement.session, &replacement.runtime));
    drop(store);
    let restored = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    assert!(allowed(&restored, &replacement.runtime));
}

#[test]
fn preparing_a_return_holds_execution_of_the_previous_replacement() {
    let sandbox = Sandbox::new();
    let mut store = RoutingStore::load(&sandbox.state.join("subscriptions/routes.json")).unwrap();
    let source = binding("claude");
    let replacement = binding("codex");
    store.track(source.clone(), "/projects/demo", "1797cdd6", 100).unwrap();
    let transition = store.reconcile(&[observation(&source, Exhausted)], 101).unwrap().remove(0);
    store.mark_retired(&source.session, &transition.generation).unwrap();
    store.mark_submitted(&source.session, &transition.generation).unwrap();
    store.record_runtime(&transition.generation, replacement.clone(), "/projects/demo").unwrap();
    store.finish(&source.session, &transition.generation).unwrap();
    store.reconcile(&[observation(&replacement, Available)], 102).unwrap();
    assert!(!store.execution_allowed(&transition.generation, &replacement.session, &replacement.runtime));
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
fn claude_status_retains_the_exact_model_id_for_subscription_return() {
    let sandbox = Sandbox::new();
    let script = repo().join("config/claude/.claude/statusline-command.sh");
    let output = sandbox.run_with_input("bash", &[script.to_str().unwrap()], &[("CLAUDE_EFFORT", "xhigh")],
        &json!({"session_id":SESSION_A,"cwd":"/projects/demo","model":{"id":"claude-opus-4-7","display_name":"Opus"}}).to_string());
    assert!(output.status.success(), "{}", stderr(&output));
    let cache: Value = serde_json::from_slice(&fs::read(sandbox.home.join(format!(".cache/claude/statusline-{SESSION_A}.json"))).unwrap()).unwrap();
    assert_eq!(cache["model_id"], "claude-opus-4-7");
    assert_eq!(cache["model"], "Opus");
    assert_eq!(cache["effort"], "xhigh");
}

#[test]
fn the_native_prompt_gate_stops_on_unavailable_or_refused_admission() {
    for reply in [None, Some("refused: ownership transfer is pending")] {
        let sandbox = Sandbox::new();
        let _daemon = reply.map(|reply| ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), reply));
        let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["gate", &"a".repeat(32)], &[]);
        assert!(output.status.success(), "{}", stderr(&output));
        let decision: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(decision["continue"], false);
    }
}

#[test]
fn the_native_prompt_gate_releases_only_after_an_explicit_host_grant() {
    let sandbox = Sandbox::new();
    let daemon = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let generation = "b".repeat(32);
    let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["gate", &generation], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), json!({}));
    let requests = daemon.received(std::time::Duration::from_secs(1));
    let payload: Value = serde_json::from_str(requests[0].strip_prefix("subscription-gate ").unwrap()).unwrap();
    assert_eq!(payload, json!({"generation":generation}));
}

#[test]
fn native_bootstrap_waits_for_the_ownership_gate_before_releasing_the_first_prompt() {
    let sandbox = Sandbox::new();
    let _daemon = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["await-gate", &"a".repeat(32)], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), json!({}));
}

fn prepared_claude_route() -> (Sandbox, cockpit::subscription_fallback::Transition) {
    let sandbox = claude_history(&[message("user", "Finish this task without pushing")]);
    let directory = sandbox.root.to_str().unwrap();
    fs::write(sandbox.runtime.join("cockpit.json"), json!({"sessions":[{
        "session":SESSION_A,"agent":"claude","state":"waiting","cwd":directory,"task":"Finish this task"
    }]}).to_string()).unwrap();
    let path = sandbox.state.join("cockpit/subscriptions/routes.json");
    let mut store = RoutingStore::load(&path).unwrap();
    let source = binding("claude");
    store.track(source.clone(), directory, "1797cdd6", 100).unwrap();
    let mut observed = observation(&source, Exhausted);
    observed.session.cwd = directory.into();
    let transition = store.reconcile(&[observed], 101).unwrap().remove(0);
    drop(store);
    (sandbox, transition)
}

#[test]
fn native_launch_plans_are_private_and_come_only_from_a_pending_route() {
    let (sandbox, transition) = prepared_claude_route();
    let directory = sandbox.root.to_str().unwrap();
    let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["prepare-plan",SESSION_A,&transition.generation], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let plan_path = std::path::PathBuf::from(stdout(&output).trim());
    assert!(plan_path.starts_with(sandbox.runtime.join("cockpit-answers")));
    let plan: Value = serde_json::from_slice(&fs::read(&plan_path).unwrap()).unwrap();
    assert_eq!(plan["generation"], transition.generation);
    assert_eq!(plan["selection"]["model"], "gpt-6.1-sol");
    assert_eq!(plan["directory"], directory);
    assert_eq!(plan["context"]["latest_user_request"], "Finish this task without pushing");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(&plan_path).unwrap().permissions().mode() & 0o777, 0o600);
    let repeated = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["prepare-plan",SESSION_A,&transition.generation], &[]);
    assert!(repeated.status.success(), "{}", stderr(&repeated));
    assert_eq!(stdout(&repeated), stdout(&output));
    let invalid = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["prepare-plan",SESSION_A,&"f".repeat(32)], &[]);
    assert!(!invalid.status.success());
    assert_eq!(fs::read_dir(plan_path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn a_published_plan_survives_source_exit_after_retirement_intent() {
    let (sandbox, transition) = prepared_claude_route();
    let binary = env!("CARGO_BIN_EXE_cockpit-subscription");
    let arguments = ["prepare-plan",SESSION_A,&transition.generation];
    let first = sandbox.run(binary,&arguments,&[]);
    assert!(first.status.success(), "{}", stderr(&first));
    let mut store = RoutingStore::load(&sandbox.state.join("cockpit/subscriptions/routes.json")).unwrap();
    store.mark_retirement_started(SESSION_A,&transition.generation).unwrap();
    drop(store);
    fs::write(sandbox.runtime.join("cockpit.json"), "{\"sessions\":[]}").unwrap();
    let retry = sandbox.run(binary,&arguments,&[]);
    assert!(retry.status.success(), "{}", stderr(&retry));
    assert_eq!(stdout(&first),stdout(&retry));
}

#[test]
fn protected_plan_retries_never_replace_conflicting_or_exposed_context() {
    use std::os::unix::fs::PermissionsExt;
    for exposed in [false, true] {
        let (sandbox, transition) = prepared_claude_route();
        let arguments = ["prepare-plan", SESSION_A, &transition.generation];
        let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &arguments, &[]);
        assert!(output.status.success(), "{}", stderr(&output));
        let path = std::path::PathBuf::from(stdout(&output).trim());
        if exposed { fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(); }
        else { fs::write(&path, "conflicting-private-context").unwrap(); }
        let previous = fs::read(&path).unwrap();
        let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &arguments, &[]);
        assert!(!output.status.success());
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert!(!stderr(&output).contains("conflicting-private-context"));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
}

#[test]
fn direct_messages_queue_on_the_pending_owner_then_resolve_the_active_replacement() {
    let (sandbox, transition) = prepared_claude_route();
    let binary = env!("CARGO_BIN_EXE_cockpit-subscription");
    let target = sandbox.run(binary, &["message-target", SESSION_A], &[]);
    assert!(target.status.success(), "{}", stderr(&target));
    assert_eq!(serde_json::from_slice::<Value>(&target.stdout).unwrap(), json!({"session":SESSION_A,"held":true}));
    let queued = sandbox.run_with_input(binary, &["queue-message", SESSION_A], &[], "A user instruction during handoff");
    assert!(queued.status.success(), "{}", stderr(&queued));
    let source_queue = sandbox.state.join(format!("cockpit/prompt-queue/{SESSION_A}"));
    let first = fs::read_dir(&source_queue).unwrap().next().unwrap().unwrap().path();
    assert_eq!(fs::read_to_string(first).unwrap(), "A user instruction during handoff");
    let mut store = RoutingStore::load(&sandbox.state.join("cockpit/subscriptions/routes.json")).unwrap();
    store.mark_retired(SESSION_A, &transition.generation).unwrap();
    store.mark_submitted(SESSION_A, &transition.generation).unwrap();
    let replacement = binding("codex");
    store.record_runtime(&transition.generation, replacement.clone(), sandbox.root.to_str().unwrap()).unwrap();
    store.finish(SESSION_A, &transition.generation).unwrap();
    store.confirm_initial_admission(SESSION_A, &transition.generation).unwrap();
    store.start_initial_turn(SESSION_A,&transition.generation).unwrap();
    drop(store);
    let target = sandbox.run(binary, &["message-target", SESSION_A], &[]);
    assert!(target.status.success(), "{}", stderr(&target));
    assert_eq!(serde_json::from_slice::<Value>(&target.stdout).unwrap(), json!({"session":replacement.session,"held":false,"native_inbox":true}));
    let queued = sandbox.run_with_input(binary, &["queue-message", SESSION_A], &[], "A later user instruction");
    assert!(queued.status.success(), "{}", stderr(&queued));
    let target_queue = sandbox.state.join(format!("cockpit/prompt-queue/{}", replacement.session));
    let later = fs::read_dir(&target_queue).unwrap().next().unwrap().unwrap().path();
    assert_eq!(fs::read_to_string(later).unwrap(), "A later user instruction");
    assert_eq!(fs::read_dir(&source_queue).unwrap().count(), 1);
}

#[test]
fn subscription_task_ownership_moves_once_and_keeps_the_previous_assignment_in_its_trail() {
    use cockpit::{continuity::{Continuity, TaskSnapshot}, todo::{Entry, Item}};
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("continuity.json");
    let mut continuity = Continuity::load(&path).unwrap();
    let item = Item(json!({"id":"1797cdd6","text":"Finish the requested change","status":"doing"}).as_object().unwrap().clone());
    let task = TaskSnapshot::from_entry(&Entry::Typed(item), "owner/repo");
    let source = Session { session: SESSION_A.into(), agent: "claude".into(), cwd: "/projects/demo".into(),
        branch: "todo-1797cdd6".into(), repository: "owner/repo".into(), ..Session::default() };
    let replacement = Session { session: binding("codex").session, agent: "codex".into(), ..source.clone() };
    continuity.assign(&task, &source, 100).unwrap();
    assert!(continuity.transfer_subscription_task("1797cdd6", &source, &replacement, 101).unwrap());
    drop(continuity);
    let mut continuity = Continuity::load(&path).unwrap();
    assert!(!continuity.transfer_subscription_task("1797cdd6", &source, &replacement, 102).unwrap());
    let assignments = &continuity.tasks()["1797cdd6"].assignments;
    assert_eq!(assignments.len(), 2);
    assert!(assignments.iter().any(|assignment| assignment.session == SESSION_A && !assignment.active));
    assert_eq!(assignments.iter().filter(|assignment| assignment.active).map(|assignment| assignment.session.as_str()).collect::<Vec<_>>(), [replacement.session.as_str()]);
}

#[test]
fn typed_and_issue_assignments_follow_a_subscription_replacement_without_requeueing() {
    use cockpit::todo::{TodoList, Item};
    for issue in [false, true] {
        let key = if issue { "owner/repo#12" } else { "1797cdd6" };
        let assignment = json!({"id":key,"text":"Finish this task","status":"doing","session":SESSION_A,"updated":100});
        let mut todos = TodoList::default();
        todos.items = if issue { Vec::new() } else { vec![Item(assignment.as_object().unwrap().clone())] };
        todos.issues = if issue { json!({key:assignment}).as_object().unwrap().clone() } else { Default::default() };
        let replacement = binding("codex").session;
        assert!(todos.transfer_subscription_task(key, SESSION_A, &replacement).unwrap());
        assert!(!todos.transfer_subscription_task(key, SESSION_A, &replacement).unwrap());
        assert!(!todos.busy(SESSION_A));
        assert!(todos.busy(&replacement));
        assert_eq!(todos.status_of(key), "doing");
        assert!(!todos.is_queued(key));
        assert!(todos.transfer_subscription_task(key, SESSION_B, SESSION_A).is_err());
        assert!(todos.busy(&replacement));
    }
}

#[test]
fn a_native_return_plan_keeps_the_original_codex_thread_and_model() {
    let sandbox = claude_history(&[message("user", "Continue the unfinished change")]);
    let directory = sandbox.root.to_str().unwrap();
    let mut store = RoutingStore::load(&sandbox.state.join("cockpit/subscriptions/routes.json")).unwrap();
    let original = binding("codex");
    store.track(original.clone(), directory, "1797cdd6", 100).unwrap();
    let mut exhausted = observation(&original, Exhausted);
    exhausted.session.cwd = directory.into();
    let transition = store.reconcile(&[exhausted], 101).unwrap().remove(0);
    store.mark_retired(&original.session, &transition.generation).unwrap();
    store.mark_submitted(&original.session, &transition.generation).unwrap();
    store.record_runtime(&transition.generation, binding("claude"), directory).unwrap();
    store.finish(&original.session, &transition.generation).unwrap();
    let mut recovered = observation(&binding("claude"), Available);
    recovered.session.cwd = directory.into();
    recovered.original_capacity = Available;
    let transition = store.reconcile(&[recovered], 102).unwrap().remove(0);
    drop(store);
    fs::write(sandbox.runtime.join("cockpit.json"), json!({"sessions":[{
        "session":SESSION_A,"agent":"claude","state":"waiting","cwd":sandbox.root,"task":"Continue the change"
    }]}).to_string()).unwrap();
    let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["prepare-plan",&original.session,&transition.generation], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let plan: Value = serde_json::from_slice(&fs::read(stdout(&output).trim()).unwrap()).unwrap();
    assert_eq!(plan["native_thread"], "aaaaaaaa-0000-4000-8000-000000000000");
    assert_eq!(plan["selection"]["model"], "gpt-6.1-sol");
    assert_eq!(plan["selection"]["variant"], "high");
}

#[test]
fn claude_plans_use_a_stable_new_session_or_resume_the_exact_original_session() {
    for returning in [false, true] {
        let sandbox = Sandbox::new();
        let source = binding("codex");
        let history = json!({"thread":source.native_thread,"messages":[{"id":"user-fixture","role":"user","text":"Finish the task"}],
            "latest_user":"Finish the task","truncated":false});
        sandbox.shim("python3", &format!("printf '%s\\n' '{}'", history));
        let directory = sandbox.root.to_str().unwrap();
        fs::write(sandbox.runtime.join("cockpit.json"), json!({"sessions":[{"session":source.session,"native_thread":source.native_thread,
            "agent":"codex","state":"waiting","cwd":directory}]}).to_string()).unwrap();
        let mut store = RoutingStore::load(&sandbox.state.join("cockpit/subscriptions/routes.json")).unwrap();
        let original = if returning { binding("claude") } else { source.clone() };
        store.track(original.clone(), directory, "1797cdd6", 100).unwrap();
        if returning {
            let mut observed = observation(&original, Exhausted);
            observed.session.cwd = directory.into();
            let first = store.reconcile(&[observed], 101).unwrap().remove(0);
            store.mark_retired(&original.session, &first.generation).unwrap();
            store.mark_submitted(&original.session, &first.generation).unwrap();
            store.record_runtime(&first.generation, source.clone(), directory).unwrap();
            store.finish(&original.session, &first.generation).unwrap();
        }
        let mut observed = observation(&source, Exhausted);
        observed.session.cwd = directory.into();
        observed.original_capacity = if returning { Available } else { Exhausted };
        let next = store.reconcile(&[observed], 102).unwrap().remove(0);
        drop(store);
        let output = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["prepare-plan", &original.session, &next.generation], &[]);
        assert!(output.status.success(), "{}", stderr(&output));
        let plan: Value = serde_json::from_slice(&fs::read(stdout(&output).trim()).unwrap()).unwrap();
        assert_eq!(plan["resume"], returning);
        let id = plan["session_id"].as_str().unwrap();
        assert!(cockpit::codex::cockpit_id(id).is_ok());
        if returning { assert_eq!(id, SESSION_A); }
        else { assert_eq!(id.replace('-', ""), next.generation); }
    }
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
