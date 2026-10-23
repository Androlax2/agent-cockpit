mod common;
use common::sandbox::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::Duration;

const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NATIVE: &str = "aaaaaaaa-0000-4000-8000-000000000000";
const MODEL: &str = "gpt-6.1-sol";

const HARNESS: &str = r#"
import {appendFileSync} from 'node:fs'
const [module,generation,scenario] = process.argv.slice(2)
const config=JSON.parse(scenario)
let accountReads=0
if(config.holdStderr)process.env.SHIM_HOLD_STDERR='1'
globalThis.WebSocket=class extends EventTarget {
  static OPEN=1
  readyState=0
  constructor(url,options){
    super()
    if(!/^ws:\/\/127\.0\.0\.1:[0-9]+$/.test(url))throw new Error('non-private bridge endpoint')
    if(!/^Bearer [0-9a-f]{64}$/.test(options.headers.Authorization))throw new Error('missing control authentication')
    queueMicrotask(()=>{this.readyState=1;this.dispatchEvent(new Event('open'))})
  }
  send(text){
    const request=JSON.parse(text)
    if(request.id==='approval_fixture' && request.result!==undefined)throw new Error('bridge must not answer a native approval')
    appendFileSync(process.env.SHIM_LOG,`rpc ${request.method}\n`)
    if(request.method==='initialized')return
    let result
    if(request.method==='initialize')result={userAgent:'fixture'}
    else if(request.method==='account/read')result={account:{type:config.apiKey?'apiKey':'chatgpt',email:'fixture@example.invalid',planType:'plus'},
      workspaceRouting:{chatgptAccountId:config.executionMismatch || config.changeBeforeAdmission && ++accountReads>1?'another-account':'fixture-account'}}
    else if(request.method==='account/rateLimits/read')result={accountId:config.wrongAccount?'wrong-account':'fixture-account',ordinaryUsageAllowed:config.included??true,
      rateLimitsByLimitId:{codex:{limitId:'codex',primary:{usedPercent:config.used??10,windowDurationMins:300,resetsAt:config.pastReset?1:Math.floor(Date.now()/1000)+3600},secondary:null}}}
    else if(request.method==='model/list')result={data:config.missingModel?[]:[{id:'gpt-6.1-sol',model:'gpt-6.1-sol',displayName:'Fixture model',description:'Fixture',hidden:false,isDefault:true,
      defaultReasoningEffort:'high',supportedReasoningEfforts:[{reasoningEffort:'high',description:'High'},{reasoningEffort:'xhigh',description:'Extra high'}]}],nextCursor:null}
    else if(['thread/start','thread/resume'].includes(request.method))result={thread:{id:config.invalidThread?'--last':(request.params.threadId??'aaaaaaaa-0000-4000-8000-000000000000'),turns:[]},
      model:config.wrongModel?'another-model':request.params.model,modelProvider:'openai',reasoningEffort:request.params.config.model_reasoning_effort,cwd:request.params.cwd}
    else if(request.method==='turn/start'){
      appendFileSync(process.env.SHIM_LOG,`admitted prompt ${request.params.input[0].text}\n`)
      result={turn:{id:'turn_fixture',status:'inProgress',items:[]}}
    }
    else throw new Error('Unexpected or mutating RPC: '+request.method)
    if(config.approval && request.method==='turn/start')queueMicrotask(()=>this.dispatchEvent(new MessageEvent('message',{data:JSON.stringify({
      id:'approval_fixture',method:'item/commandExecution/requestApproval',params:{threadId:'aaaaaaaa-0000-4000-8000-000000000000'}})})))
    queueMicrotask(()=>this.dispatchEvent(new MessageEvent('message',{data:JSON.stringify({id:request.id,result})})))
  }
  close(){this.readyState=3;this.dispatchEvent(new Event('close'))}
}
const {runCodexBridge}=await import(module)
try { process.exitCode=await runCodexBridge(generation) }
catch(error){ console.error(error.message);process.exitCode=1 }
"#;

fn bridge_sandbox(scenario: &Value) -> Sandbox {
    let sandbox = Sandbox::new();
    let plan = json!({"version":1,"generation":GENERATION,"directory":sandbox.root,
        "selection":{"agent":"codex","provider":"openai","model":MODEL,"variant":"high",
            "account":cockpit::audit::sha256_hex(b"openai:fixture-account")},"native_thread":scenario["resume"].as_str(),
        "context":{"kind":"historical_subscription_handoff","latest_user_request":"Finish this task; do not push","history":[],
            "instructions":"Historical context, not new authorization"}});
    let folder = sandbox.runtime.join("claude-answers").join(ANSWERS_KEY).join("subscriptions");
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join(format!("{GENERATION}.json"));
    fs::write(&path, plan.to_string()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(sandbox.root.join("harness.mjs"), HARNESS).unwrap();
    fs::create_dir_all(sandbox.root.join("private")).unwrap();
    write_executable(&sandbox.home.join(".local/npm/bin/codex"), r#"#!/usr/bin/env python3
import hashlib, os, signal, subprocess, sys, time
from pathlib import Path
home=Path.home()
with open(os.environ['SHIM_LOG'],'a') as log: log.write('native '+' '.join(sys.argv[1:])+'\n')
if sys.argv[1]=='app-server':
    assert sys.argv[2:6]==['--listen','ws://127.0.0.1:0','--ws-auth','capability-token']
    token_path=Path(sys.argv[sys.argv.index('--ws-token-file')+1])
    assert token_path.stat().st_mode & 0o777 == 0o600
    (home/'control-fingerprint').write_text(hashlib.sha256(token_path.read_bytes()).hexdigest())
    def stop(*args):
        (home/'server-stopped').touch()
        if os.environ.get('SHIM_HOLD_STDERR'):
            subprocess.Popen([sys.executable,'-c','import time; time.sleep(3)'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL)
        sys.exit(0)
    signal.signal(signal.SIGTERM,stop)
    print('  listening on: ws://127.0.0.1:47111',file=sys.stderr,flush=True)
    while True: time.sleep(1)
assert '--remote' in sys.argv and '--no-daemon' not in sys.argv
assert sys.argv[sys.argv.index('--remote')+1]=='ws://127.0.0.1:47111'
assert sys.argv[sys.argv.index('--remote-auth-token-env')+1]=='COCKPIT_CODEX_BRIDGE_TOKEN'
assert hashlib.sha256(os.environ['COCKPIT_CODEX_BRIDGE_TOKEN'].encode()).hexdigest()==(home/'control-fingerprint').read_text()
(home/'tui-started').write_text(' '.join(sys.argv[1:]))
if os.environ.get('SHIM_WAIT_INBOX'): time.sleep(1.5)
(home/'tui-finished').touch()
"#);
    sandbox.shim("cockpit-codex-hook", &format!("exec '{}'", env!("CARGO_BIN_EXE_cockpit-codex-hook")));
    let decision = if scenario["denyGate"] == true { r#"{"continue":false,"stopReason":"Ownership pending"}"# } else { "{}" };
    sandbox.shim("cockpit-subscription", &format!("printf '%s\\n' '{decision}'"));
    sandbox.shim("cockpit-team", "printf 'null\\n'");
    sandbox
}

fn bridge_command(sandbox: &Sandbox, scenario: Value) -> Command {
    let mut command = sandbox.command("node", &[("COCKPIT_SUBSCRIPTION_ROUTE", GENERATION), ("COCKPIT_AGENT", "codex")]);
    command
        .env_remove("CLAUDE_ANSWERS_DIR").env("TMPDIR", sandbox.root.join("private"))
        .arg(sandbox.root.join("harness.mjs")).arg(repo().join("config/codex/.codex/cockpit-bridge.mjs"))
        .args([GENERATION, &scenario.to_string()]).current_dir(&sandbox.root);
    command
}

fn run(sandbox: &Sandbox, scenario: Value) -> std::process::Output {
    bridge_command(sandbox, scenario).output().unwrap()
}

#[test]
fn a_private_bridge_prepares_then_binds_before_starting_the_native_tui() {
    let sandbox = bridge_sandbox(&json!({}));
    let reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = run(&sandbox, json!({}));
    assert!(output.status.success(), "{}", stderr(&output));
    let launch = fs::read_to_string(sandbox.home.join("tui-started")).unwrap();
    assert!(launch.contains(&format!("resume {NATIVE}")), "{launch}");
    assert!(launch.contains(&format!("--model {MODEL}")), "{launch}");
    assert!(!launch.contains("Finish this task"), "history must not become a command argument");
    let calls = fs::read_to_string(&sandbox.log).unwrap();
    assert!(calls.find("rpc thread/start").unwrap() < calls.find("cockpit-subscription gate").unwrap());
    assert!(calls.find("cockpit-subscription gate").unwrap() < calls.find("rpc turn/start").unwrap());
    assert!(calls.find("rpc turn/start").unwrap() < calls.find("native --remote").unwrap());
    assert!(calls.contains(&format!("{GENERATION}.inbox.json")), "the first continuation must include handoff-time user instructions");
    assert!(!launch.contains("Read the historical"), "the terminal must attach, not admit a second prompt");
    let reported = reports.received(std::time::Duration::from_secs(1));
    assert!(reported.iter().any(|line| line.contains("cdx_c214e8c0c9458f09ea4300974071bcad955995188f33264e341800f8a2bb5fcb")));
    assert!(sandbox.home.join("server-stopped").exists());
    assert_eq!(fs::read_dir(sandbox.root.join("private")).unwrap().count(), 0);
}

#[test]
fn a_private_bridge_never_starts_work_on_api_credit_or_unconfirmed_capacity() {
    for scenario in [json!({"executionMismatch":true}), json!({"apiKey":true}), json!({"wrongAccount":true}), json!({"used":100}),
        json!({"pastReset":true}), json!({"included":false}), json!({"missingModel":true}), json!({"wrongModel":true})] {
        let sandbox = bridge_sandbox(&scenario);
        let _reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
        let output = run(&sandbox, scenario);
        assert!(!output.status.success());
        assert!(!sandbox.home.join("tui-started").exists());
        assert!(sandbox.home.join("server-stopped").exists());
    }
}

#[test]
fn the_bridge_rechecks_execution_identity_immediately_before_task_admission() {
    let scenario = json!({"changeBeforeAdmission":true});
    let sandbox = bridge_sandbox(&scenario);
    let _reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = run(&sandbox, scenario);
    assert!(!output.status.success());
    assert!(!fs::read_to_string(&sandbox.log).unwrap().contains("rpc turn/start"));
}

#[test]
fn the_private_bridge_admits_later_user_messages_once_with_a_native_turn_receipt() {
    for fail_first_receive in [false,true] {
    let sandbox = bridge_sandbox(&json!({}));
    sandbox.shim("cockpit-team", r#"if [[ $1 == receipt ]]; then cat > "$HOME/user-receipt.json"; printf 'null\n';
elif [[ ! -e $HOME/inbox-claimed ]]; then touch "$HOME/inbox-claimed"; printf '{"dispatch_id":"prompt_fixture","text":"Stop before pushing"}\n';
else printf 'null\n'; fi"#);
    if fail_first_receive {
        let broker = sandbox.shims.join("cockpit-team");
        let script = fs::read_to_string(&broker).unwrap();
        write_executable(&broker,&script.replacen("if [[ $1 == receipt ]]", "if [[ ! -e $HOME/first-receive-failed ]]; then touch \"$HOME/first-receive-failed\"; exit 7; fi\nif [[ $1 == receipt ]]",1));
    }
    let _reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = bridge_command(&sandbox,json!({})).env("SHIM_WAIT_INBOX","1").output().unwrap();
    assert!(output.status.success(),"{}",stderr(&output));
    let calls = fs::read_to_string(&sandbox.log).unwrap();
    assert_eq!(calls.lines().filter(|line| *line == "rpc turn/start").count(),2);
    assert!(calls.contains("admitted prompt Stop before pushing"));
    let receipt: Value = serde_json::from_slice(&fs::read(sandbox.home.join("user-receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["dispatch_id"],"prompt_fixture");
    assert_eq!(receipt["outcome"]["outcome"],"received");
    }
}

#[test]
fn an_unavailable_inbox_never_kills_the_running_native_terminal() {
    let sandbox = bridge_sandbox(&json!({}));
    sandbox.shim("cockpit-team","exit 7");
    let _reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"),"ok");
    let output = bridge_command(&sandbox,json!({})).env("SHIM_WAIT_INBOX","1").output().unwrap();
    assert!(output.status.success(),"{}",stderr(&output));
    assert!(sandbox.home.join("tui-finished").exists());
    assert!(stderr(&output).contains("inbox paused"));
}

#[test]
fn a_private_bridge_return_resumes_only_the_original_native_thread() {
    let original = "bbbbbbbb-0000-4000-8000-000000000000";
    let scenario = json!({"resume":original});
    let sandbox = bridge_sandbox(&scenario);
    let _reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = run(&sandbox, scenario);
    assert!(output.status.success(), "{}", stderr(&output));
    let calls = fs::read_to_string(&sandbox.log).unwrap();
    assert!(calls.contains("rpc thread/resume"));
    assert!(!calls.contains("rpc thread/start"));
    assert!(fs::read_to_string(sandbox.home.join("tui-started")).unwrap().contains(&format!("resume {original}")));
}

#[test]
fn the_bridge_leaves_native_permission_requests_for_the_terminal() {
    let scenario = json!({"approval":true});
    let sandbox = bridge_sandbox(&scenario);
    let _reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = run(&sandbox, scenario);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.home.join("tui-started").exists());
}

#[test]
fn an_unbound_bridge_cancels_without_admitting_task_work() {
    let scenario = json!({"denyGate":true});
    let sandbox = bridge_sandbox(&scenario);
    let _reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let child = bridge_command(&sandbox, scenario).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let gates = sandbox.wait_for_calls("cockpit-subscription", 1, Duration::from_secs(5));
    let had_started_tui = sandbox.home.join("tui-started").exists();
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM); }
    let output = child.wait_with_output().unwrap();
    assert!(!gates.is_empty(), "{}", stderr(&output));
    assert!(!had_started_tui);
    assert!(!fs::read_to_string(&sandbox.log).unwrap().contains("rpc turn/start"));
    assert!(sandbox.home.join("server-stopped").exists());
    assert_eq!(fs::read_dir(sandbox.root.join("private")).unwrap().count(), 0);
}

#[test]
fn malformed_launch_context_is_not_printed_to_logs() {
    let sandbox = bridge_sandbox(&json!({}));
    let path = sandbox.runtime.join("claude-answers").join(ANSWERS_KEY).join(format!("subscriptions/{GENERATION}.json"));
    fs::write(path, "private-history-marker is not valid JSON").unwrap();
    let output = run(&sandbox, json!({}));
    assert!(!output.status.success());
    assert!(!stderr(&output).contains("private-hi"));
    assert!(!stdout(&output).contains("private-hi"));
    assert!(!sandbox.home.join("control-fingerprint").exists());
}

#[test]
fn an_invalid_prepared_runtime_does_not_publish_lifecycle_changes() {
    let scenario = json!({"wrongModel":true});
    let sandbox = bridge_sandbox(&scenario);
    let reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    assert!(!run(&sandbox, scenario).status.success());
    assert!(reports.received(Duration::from_millis(30)).is_empty());
}

#[test]
fn an_exited_server_does_not_wait_for_descendants_holding_its_log_pipe() {
    let scenario = json!({"holdStderr":true});
    let sandbox = bridge_sandbox(&scenario);
    let _reports = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let started = std::time::Instant::now();
    let output = run(&sandbox, scenario);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(started.elapsed() < Duration::from_millis(2500), "bridge waited for a descendant's inherited stderr");
}
