mod common;
#[path = "common/codex_quota.rs"]
mod codex_quota;
use common::sandbox::*;
use serde_json::{json, Value};
use std::fs;

fn claude_fixture() -> Sandbox {
    let mut sandbox = Sandbox::new();
    for name in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_BASE_URL",
                 "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"] {
        sandbox.env.insert(name.into(), String::new());
    }
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_cockpit-subscription"), sandbox.shims.join("cockpit-subscription")).unwrap();
    fs::create_dir_all(sandbox.home.join(".claude")).unwrap();
    fs::write(sandbox.home.join(".claude/.credentials.json"), json!({"claudeAiOauth":{
        "accessToken":"private-fixture-token","expiresAt":4102444800000_i64}}).to_string()).unwrap();
    fs::write(sandbox.home.join(".claude.json"), json!({"oauthAccount":{
        "accountUuid":"account-fixture","organizationUuid":"org-fixture"}}).to_string()).unwrap();
    fs::write(sandbox.home.join("auth.json"), json!({"loggedIn":true,"authMethod":"claude.ai",
        "orgId":"org-fixture","email":"private@example.test"}).to_string()).unwrap();
    fs::write(sandbox.home.join("profile.json"), json!({"account":{"uuid":"account-fixture","email":"private@example.test"},
        "organization":{"uuid":"org-fixture","has_extra_usage_enabled":false}}).to_string()).unwrap();
    fs::create_dir_all(sandbox.home.join(".cache/claude")).unwrap();
    fs::write(sandbox.home.join(format!(".cache/claude/statusline-{SESSION_A}.json")),
        json!({"model_id":"claude-opus-4-7","effort":"high","updated":cockpit::util::now()}).to_string()).unwrap();
    write_executable(&sandbox.home.join(".local/bin/claude"), "#!/bin/sh\n[ \"$*\" = 'auth status --json' ] || exit 1\ncat \"$HOME/auth.json\"\n");
    write_executable(&sandbox.shims.join("curl"), r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
assert sys.stdin.read() == 'Authorization: Bearer private-fixture-token\n'
url=sys.argv[-1]
assert '--location' not in sys.argv
if url=='https://api.anthropic.com/api/oauth/profile':
    print((Path.home()/'profile.json').read_text())
elif url=='https://api.anthropic.com/api/oauth/usage':
    print(json.dumps({'five_hour':{'utilization':10,'resets_at':'2099-01-01T00:00:00Z'}}))
else: raise AssertionError('unexpected endpoint')
"#);
    sandbox
}

fn collect(sandbox: &Sandbox, agent: &str) -> std::process::Output {
    sandbox.command("python3", &[]).arg(waybar_scripts().join("native-subscription.py")).arg(agent).output().unwrap()
}

#[test]
fn native_claude_collector_binds_live_billing_and_quota_without_disclosing_identity() {
    let sandbox = claude_fixture();
    let output = collect(&sandbox, "claude");
    assert!(output.status.success(), "{}", stderr(&output));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["usage"]["routing_profile"]["organization"]["has_extra_usage_enabled"], false);
    assert_eq!(result["usage"]["five_hour"]["utilization"], 10);
    assert_eq!(result["account"].as_str().unwrap().len(), 64);
    for secret in ["private-fixture-token", "private@example.test", "account-fixture", "org-fixture"] {
        assert!(!stdout(&output).contains(secret));
        assert!(!stderr(&output).contains(secret));
    }
}

#[test]
fn native_claude_collector_refuses_mismatched_identity_and_non_subscription_auth() {
    for (field, value) in [("orgId", json!("another-org")), ("email", json!("another@example.test")),
                           ("authMethod", json!("api_key")), ("loggedIn", json!(false))] {
        let sandbox = claude_fixture();
        let path = sandbox.home.join("auth.json");
        let mut auth: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        auth[field] = value;
        fs::write(path, auth.to_string()).unwrap();
        let output = collect(&sandbox, "claude");
        assert!(!output.status.success());
        assert!(stdout(&output).is_empty());
    }
}

#[test]
fn native_claude_collector_preserves_enabled_and_unknown_overage_evidence() {
    for enabled in [json!(true), Value::Null] {
        let sandbox = claude_fixture();
        let path = sandbox.home.join("profile.json");
        let mut profile: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        profile["organization"]["has_extra_usage_enabled"] = enabled.clone();
        fs::write(path, profile.to_string()).unwrap();
        let output = collect(&sandbox, "claude");
        assert!(output.status.success(), "{}", stderr(&output));
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["usage"]["routing_profile"]["organization"]["has_extra_usage_enabled"], enabled);
    }
}

#[test]
fn launch_identity_refuses_authentication_overrides_instead_of_claiming_the_host_account() {
    let sandbox = claude_fixture();
    let script = waybar_scripts().join("native-subscription.py");
    let output = sandbox.command("python3",&[]).arg(&script).args(["identity","claude"]).current_dir(&sandbox.home).output().unwrap();
    assert!(output.status.success(),"{}",stderr(&output));
    assert_eq!(stdout(&output).trim(),cockpit::audit::sha256_hex(b"anthropic:org-fixture:account-fixture"));
    for name in ["ANTHROPIC_AUTH_TOKEN","CLAUDE_CONFIG_DIR","ANTHROPIC_BASE_URL","CLAUDE_CODE_OAUTH_TOKEN"] {
        let output = sandbox.command("python3",&[]).arg(&script).args(["identity","claude"]).current_dir(&sandbox.home).env(name,"unverified-fixture").output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    for settings in [json!({"apiKeyHelper":"unverified-helper"}),json!({"env":{"ANTHROPIC_API_KEY":"unverified-key"}})] {
        fs::write(sandbox.home.join(".claude/settings.json"),settings.to_string()).unwrap();
        let output = sandbox.command("python3",&[]).arg(&script).args(["identity","claude"]).current_dir(&sandbox.home).output().unwrap();
        assert!(!output.status.success(),"project/user settings must not silently select another billing source");
    }
}

#[test]
fn claude_candidate_discovery_uses_an_isolated_blocked_prompt_without_real_credentials() {
    let sandbox = claude_fixture();
    write_executable(&sandbox.home.join(".local/bin/claude"),r#"#!/usr/bin/env python3
import json,os,sys
assert os.environ['ANTHROPIC_API_KEY']=='sk-ant-metadata-only-not-a-real-key'
assert os.environ['ANTHROPIC_BASE_URL']=='http://127.0.0.1:1'
assert os.environ['HOME']!=os.environ['ORIGINAL_FIXTURE_HOME']
assert '--effort' in sys.argv and sys.argv[sys.argv.index('--effort')+1]=='high'
settings=json.loads(sys.argv[sys.argv.index('--settings')+1])
assert 'continue' in settings['hooks']['UserPromptSubmit'][0]['hooks'][0]['command']
print(json.dumps({'type':'system','subtype':'init','model':'claude-opus-fixture'}))
"#);
    let output = sandbox.command("python3",&[]).arg(waybar_scripts().join("native-subscription.py")).arg("discover-claude")
        .env("ORIGINAL_FIXTURE_HOME",&sandbox.home).output().unwrap();
    assert!(output.status.success(),"{}",stderr(&output));
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(),json!({"model":"claude-opus-fixture","effort":"high"}));
}

#[test]
fn a_fresh_native_claude_candidate_does_not_require_an_unrelated_enrolled_route() {
    let sandbox = claude_fixture();
    write_executable(&sandbox.home.join(".local/bin/claude"),r#"#!/usr/bin/env python3
import json,sys
from pathlib import Path
if sys.argv[1:]==['auth','status','--json']:
    print((Path.home()/'auth.json').read_text())
else:
    print(json.dumps({'type':'system','subtype':'init','model':'claude-opus-fixture'}))
"#);
    let output = sandbox.command(env!("CARGO_BIN_EXE_cockpit-subscription"),&[]).args(["candidate","claude"])
        .arg(&sandbox.home).output().unwrap();
    assert!(output.status.success(),"{}",stderr(&output));
    let candidate: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(candidate["model"],"claude-opus-fixture");
    assert_eq!(candidate["variant"],"high");
    assert_eq!(candidate["account"],source_row(&sandbox)["subscription_account"]);
}

fn codex_fixture() -> Sandbox {
    let sandbox = codex_quota::sandbox();
    fs::create_dir_all(sandbox.home.join("normal-codex")).unwrap();
    fs::write(sandbox.home.join("quota-response.json"), json!({"accountId":"fixture-workspace","ordinaryUsageAllowed":true,
        "rateLimits":{"primary":{"usedPercent":20,"windowDurationMins":300,"resetsAt":4102444800_i64}}}).to_string()).unwrap();
    fs::write(sandbox.home.join("account-response.json"), json!({"account":{"type":"chatgpt"},
        "workspaceRouting":{"chatgptAccountId":"fixture-workspace"}}).to_string()).unwrap();
    codex_quota::vendor(&sandbox, r#"
import json, os, sys
from pathlib import Path
assert Path(os.environ['CODEX_HOME']) == Path.home() / 'normal-codex'
assert sys.argv[1:] == ['app-server']
for line in sys.stdin:
    message=json.loads(line)
    method=message['method']
    with open(os.environ['SHIM_LOG'], 'a') as log: log.write(method+'\n')
    if method=='initialized': continue
    if method=='initialize': result={}
    elif method=='account/read': result=json.loads((Path.home()/'account-response.json').read_text())
    elif method=='account/rateLimits/read': result=json.loads((Path.home()/'quota-response.json').read_text())
    elif method=='model/list': result={'data':[{'model':'gpt-fixture','hidden':False,'isDefault':True,
        'defaultReasoningEffort':'high','supportedReasoningEfforts':[{'reasoningEffort':'high'}]}],'nextCursor':None}
    else: raise AssertionError('forbidden method '+method)
    print(json.dumps({'id':message['id'],'result':result}),flush=True)
"#);
    sandbox
}

#[test]
fn native_codex_collector_uses_execution_home_and_backend_bound_catalog() {
    let sandbox = codex_fixture();
    let output = collect(&sandbox, "codex");
    assert!(output.status.success(), "{}", stderr(&output));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["account"], "df9e9e72b3bc12700495d3e62d855e093227025065df23af8e3e7e22fc77f4f9");
    assert_eq!(result["usage"]["ordinary_usage_allowed"], true);
    assert_eq!(result["models"][0]["model"], "gpt-fixture");
    assert!(!stdout(&output).contains("fixture-workspace"));
    assert!(!fs::read_to_string(&sandbox.log).unwrap().contains("turn/start"));
}

#[test]
fn native_codex_collector_refuses_an_execution_account_mismatch() {
    let sandbox = codex_fixture();
    fs::write(sandbox.home.join("account-response.json"), json!({"account":{"type":"chatgpt"},
        "workspaceRouting":{"chatgptAccountId":"different-workspace"}}).to_string()).unwrap();
    let output = collect(&sandbox, "codex");
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
}

fn native_command(sandbox: &Sandbox, row: Value, arguments: &[&str]) -> std::process::Output {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    use std::time::{Duration, Instant};
    let folder = sandbox.runtime.join("claude-team-host");
    fs::create_dir_all(&folder).unwrap();
    let listener = UnixListener::bind(folder.join("host.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let retired = sandbox.home.join("retired");
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !stopped.load(Ordering::Relaxed) && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else { std::thread::sleep(Duration::from_millis(5)); continue };
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["request"], "subscription_source");
            assert_eq!(request["session"], SESSION_A);
            let value = if retired.exists() { Value::Null } else { row.clone() };
            stream.write_all(json!({"request_id":SESSION_A,"result":{"Ok":value}}).to_string().as_bytes()).unwrap();
        }
    });
    let output = sandbox.command(env!("CARGO_BIN_EXE_cockpit-subscription"), &[]).args(arguments).output().unwrap();
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    output
}

fn source_row(sandbox: &Sandbox) -> Value {
    json!({"session":SESSION_A,"agent":"claude","runtime":"host-runtime-fixture", "cwd":sandbox.home,
        "state":"waiting","tmux_pane":"%1","started":1,
        "subscription_account":cockpit::audit::sha256_hex(b"anthropic:org-fixture:account-fixture")})
}

#[test]
fn source_inspection_refuses_quota_for_another_execution_account() {
    let sandbox = claude_fixture();
    let cache = sandbox.home.join(".cache/claude");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join(format!("statusline-{SESSION_A}.json")),json!({"model_id":"claude-opus-4-7","effort":"high","updated":cockpit::util::now()}).to_string()).unwrap();
    write_executable(&sandbox.shims.join("tmux"), "#!/bin/sh\ncase $1 in\n display-message) echo '2 5 0 claude-retry-restart-123';;\n capture-pane) printf '❯ \\n';;\nesac\n");
    let mut row = source_row(&sandbox);
    row["subscription_account"] = json!("f".repeat(64));
    let output = native_command(&sandbox, row, &["inspect",SESSION_A]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("execution account"), "{}", stderr(&output));
}

#[test]
fn source_inspection_retains_exact_native_model_effort_and_authenticated_runtime() {
    for effort in [Some("high"), None] {
        let sandbox = claude_fixture();
        write_executable(&sandbox.shims.join("tmux"), "#!/bin/sh\ncase \"$1\" in\n display-message) printf '2 5 0 claude-retry-restart-123\\n';;\n capture-pane) printf '❯ \\n';;\n *) exit 1;;\nesac\n");
        let cache = sandbox.home.join(".cache/claude");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join(format!("statusline-{SESSION_A}.json")), json!({"model":"Display label",
            "model_id":"claude-opus-4-7","effort":effort,"updated":cockpit::util::now()}).to_string()).unwrap();
        let output = native_command(&sandbox, source_row(&sandbox), &["inspect", SESSION_A]);
        if effort.is_none() { assert!(!output.status.success()); continue; }
        assert!(output.status.success(), "{}", stderr(&output));
        let source: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(source["binding"]["selection"]["model"], "claude-opus-4-7");
        assert_eq!(source["binding"]["selection"]["variant"], "high");
        assert_eq!(source["binding"]["runtime"], "host-runtime-fixture");
        assert_eq!(source["prompt_clear"], true);
    }
}

#[test]
fn retirement_refuses_busy_or_drafted_sources_and_waits_for_verified_process_exit() {
    use cockpit::subscription_fallback::{Binding, Observation, Phase, RoutingStore, Selection, Stage};
    use cockpit::usage::SubscriptionCapacity::{Available, Exhausted};
    for (state, draft, may_retire) in [("waiting", false, true), ("working", false, false), ("waiting", true, false), ("needs-you", false, false), ("changed-model",false,false)] {
        let sandbox = claude_fixture();
        write_executable(&sandbox.shims.join("tmux"), &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$SHIM_LOG\"\ncase \"$1\" in\n display-message) printf '2 5 0 claude-retry-restart-123\\n';;\n capture-pane) printf '❯ {}\\n';;\n select-pane) exit 0;;\n kill-session) touch \"$HOME/retired\";;\n *) exit 1;;\nesac\n", if draft { "unsent draft" } else { "" }));
        let mut row = source_row(&sandbox);
        let session = cockpit::sessions::Session::from_value(&row).unwrap();
        let selection = Selection { agent:"claude".into(),provider:"anthropic".into(),model:"claude-opus-4-7".into(),variant:Some("high".into()),account:source_row(&sandbox)["subscription_account"].as_str().unwrap().into() };
        let source = Binding { session:SESSION_A.into(),runtime:"host-runtime-fixture".into(),selection,native_thread:None };
        let mut store = RoutingStore::load(&sandbox.state.join("cockpit/subscriptions/routes.json")).unwrap();
        store.track(source.clone(), sandbox.home.to_str().unwrap(), "1797cdd6", 100).unwrap();
        let alternate = Selection { agent:"codex".into(),provider:"openai".into(),model:"gpt-fixture".into(),variant:Some("high".into()),account:"b".repeat(64) };
        let transition = store.reconcile(&[Observation { binding:source,session,current_capacity:Exhausted,original_capacity:Exhausted,
            alternate:Some((alternate,Available)),has_pending_work:true,prompt_clear:true,manual_override:false }], 101).unwrap().remove(0);
        if state == "working" { store.mark_retirement_started(SESSION_A,&transition.generation).unwrap(); }
        drop(store);
        row["state"] = json!(if state == "changed-model" {"waiting"} else {state});
        if state == "changed-model" {
            fs::write(sandbox.home.join(format!(".cache/claude/statusline-{SESSION_A}.json")),
                json!({"model_id":"claude-sonnet-4-6","effort":"high","updated":cockpit::util::now()}).to_string()).unwrap();
        }
        let output = native_command(&sandbox, row, &["retire", SESSION_A, &transition.generation]);
        assert_eq!(output.status.success(), may_retire, "{}", stderr(&output));
        assert_eq!(sandbox.home.join("retired").exists(), may_retire);
        let store = RoutingStore::load(&sandbox.state.join("cockpit/subscriptions/routes.json")).unwrap();
        let Phase::Pending(pending) = &store.route(SESSION_A).unwrap().phase else { panic!("lost transition") };
        assert_eq!(pending.stage, if may_retire { Stage::Retired } else { Stage::Prepared });
        if !may_retire && draft { assert!(fs::read_to_string(&sandbox.log).unwrap().contains("select-pane -e")); }
        if state == "working" { assert!(fs::read_to_string(&sandbox.log).unwrap().contains("select-pane -e"), "recovery must restore input disabled by the previous dispatcher"); }
    }
}

#[test]
fn retirement_recovers_a_confirmed_source_exit_after_durable_retirement_intent() {
    use cockpit::subscription_fallback::{Binding, Observation, RoutingStore, Selection};
    use cockpit::usage::SubscriptionCapacity::{Available, Exhausted};
    let sandbox = claude_fixture();
    let source = Binding { session:SESSION_A.into(),runtime:"host-runtime-fixture".into(),native_thread:None,
        selection:Selection { agent:"claude".into(),provider:"anthropic".into(),model:"claude-opus-4-7".into(),variant:Some("high".into()),account:"a".repeat(64) } };
    let path = sandbox.state.join("cockpit/subscriptions/routes.json");
    let mut store = RoutingStore::load(&path).unwrap();
    store.track(source.clone(), sandbox.home.to_str().unwrap(), "1797cdd6", 100).unwrap();
    let alternate = Selection { agent:"codex".into(),provider:"openai".into(),model:"gpt-fixture".into(),variant:Some("high".into()),account:"b".repeat(64) };
    let transition = store.reconcile(&[Observation { binding:source,session:cockpit::sessions::Session::from_value(&source_row(&sandbox)).unwrap(),
        current_capacity:Exhausted,original_capacity:Exhausted,alternate:Some((alternate,Available)),has_pending_work:true,prompt_clear:true,manual_override:false }],101).unwrap().remove(0);
    drop(store);
    let mut persisted: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    persisted["routes"][SESSION_A]["phase"]["Pending"]["retirement_started"] = json!(true);
    fs::write(&path, persisted.to_string()).unwrap();
    fs::write(sandbox.home.join("retired"), "").unwrap();
    let output = native_command(&sandbox, source_row(&sandbox), &["retire",SESSION_A,&transition.generation]);
    assert!(output.status.success(), "{}", stderr(&output));
    let status = sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["route-status",SESSION_A,&transition.generation], &[]);
    assert_eq!(serde_json::from_slice::<Value>(&status.stdout).unwrap()["stage"], "Retired");
}

#[test]
fn routed_claude_launch_uses_the_protected_exact_selection_only_with_included_billing() {
    use std::os::unix::fs::PermissionsExt;
    for (enabled, matches_account, launches) in [(false,true,true),(true,true,false),(false,false,false)] {
        let sandbox = claude_fixture();
        let evidence: Value = serde_json::from_slice(&collect(&sandbox, "claude").stdout).unwrap();
        let generation = "a".repeat(32);
        let folder = sandbox.runtime.join("cockpit-answers").join(ANSWERS_KEY).join("subscriptions");
        fs::create_dir_all(&folder).unwrap();
        let plan = json!({"version":1,"generation":generation,"directory":sandbox.home,"session_id":SESSION_A,
            "selection":{"agent":"claude","provider":"anthropic","model":"claude-opus-4-7","variant":"high",
                "account":if matches_account { evidence["account"].clone() } else { json!("b".repeat(64)) }},
            "resume":true,"context":{"kind":"historical_subscription_handoff","latest_user_request":"fixture request"}});
        let path = folder.join(format!("{generation}.json"));
        fs::write(&path, plan.to_string()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let profile_path = sandbox.home.join("profile.json");
        let mut profile: Value = serde_json::from_str(&fs::read_to_string(&profile_path).unwrap()).unwrap();
        profile["organization"]["has_extra_usage_enabled"] = json!(enabled);
        fs::write(profile_path, profile.to_string()).unwrap();
        let launcher = sandbox.home.join(".local/bin/greywall");
        write_executable(&launcher, "#!/usr/bin/env python3\nimport json,os,sys\nassert not os.environ.get('ANTHROPIC_API_KEY')\nopen(os.environ['HOME']+'/launched.json','w').write(json.dumps(sys.argv[1:]))\n");
        let output = sandbox.command("python3", &[]).arg(waybar_scripts().join("native-subscription.py"))
            .args(["launch-claude", &generation, launcher.to_str().unwrap(), "--profile", "claude"])
            .arg("--").arg(sandbox.home.join(".local/bin/claude"))
            .env("CLAUDE_ANSWERS_KEY", ANSWERS_KEY).env("COCKPIT_SUBSCRIPTION_ROUTE", &generation)
            .current_dir(&sandbox.home).output().unwrap();
        assert_eq!(output.status.success(), launches, "{}", stderr(&output));
        assert_eq!(sandbox.home.join("launched.json").exists(), launches);
        if launches {
            let args: Value = serde_json::from_str(&fs::read_to_string(sandbox.home.join("launched.json")).unwrap()).unwrap();
            let args = args.as_array().unwrap();
            assert!(args.windows(2).any(|pair| pair == [json!("--model"),json!("claude-opus-4-7")]));
            assert!(args.windows(2).any(|pair| pair == [json!("--effort"),json!("high")]));
            assert!(args.windows(2).any(|pair| pair == [json!("--resume"),json!(SESSION_A)]));
            assert!(!args.iter().any(|arg| arg.as_str().is_some_and(|arg| arg.contains("fixture request"))));
            assert!(args.iter().any(|arg| arg.as_str().is_some_and(|arg| arg.contains(&format!("{generation}.inbox.json")))));
        }
    }
}

#[test]
fn native_launch_tickets_are_persisted_once_after_retirement_and_fresh_target_checks() {
    use cockpit::subscription_fallback::{Binding, Observation, RoutingStore, Selection};
    use cockpit::usage::SubscriptionCapacity::{Available, Exhausted};
    let sandbox = codex_fixture();
    let evidence: Value = serde_json::from_slice(&collect(&sandbox, "codex").stdout).unwrap();
    let source = Binding { session:SESSION_A.into(),runtime:"host-runtime".into(),native_thread:None,
        selection:Selection { agent:"claude".into(),provider:"anthropic".into(),model:"claude-opus-4-7".into(),variant:Some("high".into()),account:"a".repeat(64) } };
    let alternate = Selection { agent:"codex".into(),provider:"openai".into(),model:"gpt-fixture".into(),variant:Some("high".into()),
        account:evidence["account"].as_str().unwrap().into() };
    let path = sandbox.state.join("cockpit/subscriptions/routes.json");
    let mut store = RoutingStore::load(&path).unwrap();
    store.track(source.clone(), sandbox.home.to_str().unwrap(), "1797cdd6", 100).unwrap();
    let session = cockpit::sessions::Session::from_value(&source_row(&sandbox)).unwrap();
    let transition = store.reconcile(&[Observation { binding:source,session,current_capacity:Exhausted,original_capacity:Exhausted,
        alternate:Some((alternate,Available)),has_pending_work:true,prompt_clear:true,manual_override:false }], 101).unwrap().remove(0);
    drop(store);
    let arguments = ["submit", SESSION_A, &transition.generation];
    let binary = env!("CARGO_BIN_EXE_cockpit-subscription");
    assert!(!sandbox.run(binary, &arguments, &[]).status.success());
    let mut store = RoutingStore::load(&path).unwrap();
    store.mark_retired(SESSION_A, &transition.generation).unwrap();
    drop(store);
    let output = sandbox.run(binary, &arguments, &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let ticket: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(ticket["agent"], "codex");
    assert_eq!(ticket["directory"], sandbox.home.display().to_string());
    assert!(!sandbox.run(binary, &arguments, &[]).status.success());
}

#[test]
fn native_observation_enrolls_an_assigned_task_and_prepares_only_confirmed_exhaustion() {
    for kind in ["owner","teammate","ordinary"] {
    let teammate = kind == "teammate";
    let ordinary = kind == "ordinary";
    for command in ["observe", "sweep"] {
    for exhausted in [false, true] {
        let sandbox = claude_fixture();
        let mut row = source_row(&sandbox);
        row["subscription_account"] = json!("a".repeat(64));
        row["quota_stopped"] = json!(ordinary && exhausted);
        if teammate { row["team_task"] = json!("1797cdd6"); }
        fs::write(sandbox.runtime.join("cockpit.json"), json!({"sessions":[row]}).to_string()).unwrap();
        sandbox.shim("cockpit", "exit 0");
        write_executable(&sandbox.shims.join("tmux"), "#!/bin/sh\ncase $1 in\n display-message) echo '2 5 0 claude-retry-restart-123';;\n capture-pane) printf '❯ \\n';;\nesac\n");
        let cache = sandbox.home.join(".cache/claude");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join(format!("statusline-{SESSION_A}.json")), json!({"model_id":"claude-opus-4-7","effort":"high","updated":cockpit::util::now()}).to_string()).unwrap();
        fs::create_dir_all(sandbox.state.join("cockpit")).unwrap();
        fs::write(sandbox.state.join("cockpit/todo.json"), json!({"items":[{"id":"1797cdd6","text":"Finish the task","status":"doing","session":if teammate {SESSION_B} else {SESSION_A}}]}).to_string()).unwrap();
        if ordinary { fs::write(sandbox.state.join("cockpit/todo.json"),"{\"items\":[]}").unwrap(); }
        // The provider boundary is external; the CLI still inspects the host runtime and exact selection.
        write_executable(&sandbox.shims.join("python3"), &format!(r#"#!/bin/sh
case "$2" in
 claude) printf '{{"account":"{}","fetched":%s,"usage":{{"five_hour":{{"utilization":{},"resets_at":"2099-01-01T00:00:00Z"}},"routing_profile":{{"organization":{{"has_extra_usage_enabled":false}}}}}}}}\n' "$(date +%s)";;
 codex) printf '{{"account":"{}","fetched":%s,"usage":{{"rate_limit":{{"primary_window":{{"used_percent":10,"reset_at":4102444800}}}},"ordinary_usage_allowed":true}},"models":[{{"model":"gpt-fixture","isDefault":true,"defaultReasoningEffort":"high","supportedReasoningEfforts":[{{"reasoningEffort":"high"}}]}}]}}\n' "$(date +%s)";;
 *) exit 1;;
esac
"#, "a".repeat(64), if exhausted {100} else {10}, "b".repeat(64)));
        let arguments = if command == "sweep" { vec![command] } else { vec![command, SESSION_A] };
        let output = native_command(&sandbox, row, &arguments);
        assert!(output.status.success(), "{}", stderr(&output));
        let routes = cockpit::subscription_fallback::read_routes(&sandbox.state.join("cockpit/subscriptions/routes.json")).unwrap();
        assert_eq!(routes.len(), usize::from(!ordinary || exhausted));
        if ordinary && !exhausted { continue; }
        assert_eq!(routes[0].task, if ordinary {format!("native-session:{SESSION_A}")} else {"1797cdd6".into()});
        assert_eq!(matches!(routes[0].phase, cockpit::subscription_fallback::Phase::Pending(_)), exhausted);
        assert_eq!(sandbox.calls("cockpit").iter().filter(|call| call.contains("subscription-route")).count(),
            usize::from(command == "sweep" && exhausted));
    }
    }
    }
}
