use super::*;
use std::io::{Read,Write};
use std::os::unix::{fs::{PermissionsExt,symlink},net::UnixListener};
use std::process::{Command,Stdio};
use std::time::{Duration,Instant};

fn executable(path: &Path, text: &str) {
    fs::write(path,text).unwrap();
    fs::set_permissions(path,fs::Permissions::from_mode(0o700)).unwrap();
}

fn run_host_command(bench: &mut Bench, listener: &UnixListener, arguments: &[&str], environment: &[(&str,String)]) {
    let root = bench.fixture.root.clone();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let errors = fs::File::create(root.join("roundtrip-stderr")).unwrap();
    let mut command = Command::new(repository.join("config/bin/.local/bin/cockpit"));
    command.args(arguments).envs(environment.iter().map(|(key,value)| (*key,value)))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(errors);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now()+Duration::from_secs(40);
    loop {
        if root.join("retired").exists() {
            let mut world = bench.world.borrow_mut();
            world.sender_runtimes.remove(&10);
            world.sender_runtimes.remove(&40);
            fs::remove_file(root.join("retired")).unwrap();
        }
        if root.join("launched.json").exists() {
            let launch: Value = serde_json::from_slice(&fs::read(root.join("launched.json")).unwrap()).unwrap();
            fs::remove_file(root.join("launched.json")).unwrap();
            let agent = launch["agent"].as_str().unwrap();
            let route = cockpit::subscription_fallback::read_routes(&cockpit::paths::subscription_routes()).unwrap().remove(0);
            let cockpit::subscription_fallback::Phase::Pending(pending) = route.phase else {panic!("launch without a pending route")};
            let generation = launch["generation"].as_str().unwrap();
            let session = if agent == "codex" {CODEX_SESSION.to_string()}
                else if pending.direction == cockpit::subscription_fallback::Direction::Return {route.original.session}
                else {format!("{}-{}-{}-{}-{}",&generation[..8],&generation[8..12],&generation[12..16],&generation[16..20],&generation[20..])};
            let is_return = root.join("recovered").exists();
            let (peer,runtime) = if is_return {(11,"runtime-return")} else {(40,"runtime-40")};
            let account = pending.target.account;
            let directory = root.join("home/IdeaProjects/project-a");
            {
                let mut world = bench.world.borrow_mut();
                world.sender_runtimes.insert(peer,runtime.into());
                world.sender_targets.insert(peer,target(&[("tmux_pane",if agent == "codex" {"%40"} else {"%11"})]));
                world.sender_directories.insert(peer,directory.display().to_string());
                world.subscription_tokens.insert(peer,launch["generation"].as_str().unwrap().into());
                world.subscription_accounts.insert(runtime.into(),account);
            }
            let mut report = json!({"session":session,"agent":agent,"project":"project-a","cwd":directory,"state":"waiting"});
            if agent == "codex" { report["native_thread"] = json!(SESSION_A); }
            bench.registry.report_from(parse_session(&report.to_string()).unwrap(),peer).unwrap();
            if agent == "claude" {
                fs::write(root.join(format!("home/.cache/claude/statusline-{session}.json")),
                    json!({"model_id":pending.target.model,"effort":pending.target.variant,"updated":cockpit::util::now()}).to_string()).unwrap();
                fs::write(root.join(format!("home/.claude/projects/demo/{session}.jsonl")),
                    json!({"type":"user","message":{"content":[{"type":"text","text":"Continue only the same unfinished requested change"}]}}).to_string()).unwrap();
            }
            fs::write(root.join("current-agent"),agent).unwrap();
        }
        match listener.accept() {
            Ok((mut stream,_)) => {
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).unwrap();
                let request = serde_json::from_slice(&bytes).unwrap();
                let reply = crate::teams::host(&mut bench.registry,0,request).unwrap();
                stream.write_all(&serde_json::to_vec(&reply).unwrap()).unwrap();
            },
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(error) => panic!("host fixture socket: {error}"),
        }
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(),"{}",fs::read_to_string(root.join("roundtrip-stderr")).unwrap());
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native public-entry roundtrip timed out: {}",fs::read_to_string(root.join("roundtrip-stderr")).unwrap());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn native_manager_and_daemon_complete_a_fallback_and_exact_original_return() {
    use cockpit::subscription_fallback::{Phase,read_routes};
    for assigned in [true,false] {
    for original_agent in ["claude","codex"] {
    let mut bench = team_bench();
    let root = bench.fixture.root.clone();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let shims = root.join("shims");
    fs::create_dir_all(&shims).unwrap();
    symlink(repository.join("config/bin/.local/bin/cockpit"),shims.join("cockpit")).unwrap();
    executable(&shims.join("python3"),r#"#!/usr/bin/python3
import json,os,sys,time
from pathlib import Path
root=Path(os.environ['ROUNDTRIP_ROOT'])
original=(root/'initial-agent').read_text()
if sys.argv[1].endswith('codex-session.py'):
    print(json.dumps({'thread':sys.argv[2],'model':'gpt-fixture','provider':'openai','effort':'high',
        'messages':[{'id':'native-user','role':'user','text':'Finish the same requested change'}],
        'latest_user':'Finish the same requested change','truncated':False,
        'quota_stopped':(root/'unfinished').exists() or original=='codex' and not (root/'recovered').exists()}))
elif sys.argv[-1]=='discover-claude': print(json.dumps({'model':'claude-opus-fixture','effort':'high'}))
elif sys.argv[2]=='claude':
    print(json.dumps({'account':'a'*64,'fetched':int(time.time()),'usage':{'five_hour':{
        'utilization':100 if original=='claude' and not (root/'recovered').exists() else 10,'resets_at':'2099-01-01T00:00:00Z'},
        'routing_profile':{'organization':{'has_extra_usage_enabled':False}}}}))
elif sys.argv[2]=='codex':
    print(json.dumps({'account':'b'*64,'fetched':int(time.time()),'usage':{'ordinary_usage_allowed':True,
        'rate_limit':{'primary_window':{'used_percent':100 if original=='codex' and not (root/'recovered').exists() else 10,'reset_at':4102444800}}},'models':[{
        'model':'gpt-fixture','isDefault':True,'defaultReasoningEffort':'high','supportedReasoningEfforts':[{'reasoningEffort':'high'}]}]}))
else: raise AssertionError('unexpected provider request')
"#);
    executable(&shims.join("tmux"),r#"#!/usr/bin/python3
import json,os,sys
from pathlib import Path
root=Path(os.environ['ROUNDTRIP_ROOT'])
agent=(root/'current-agent').read_text()
args=sys.argv[1:]
if args[0]=='display-message': print('2 5 0 claude-retry-codex-fixture' if agent=='codex' else '2 5 0 claude-retry-restart-fixture')
elif args[0]=='capture-pane': print('\x1b[1m›\x1b[0m \x1b[2mAsk Codex to do anything\x1b[0m' if agent=='codex' else '❯ ')
elif args[0]=='kill-session': (root/'retired').touch()
elif args[0]=='new-session':
    generation=next(arg.split('=',1)[1] for arg in args if arg.startswith('COCKPIT_SUBSCRIPTION_ROUTE='))
    agent='codex' if any(arg.startswith('claude-retry-codex-') for arg in args) else 'claude'
    (root/'launched.json').write_text(json.dumps({'agent':agent,'generation':generation}))
elif args[0] not in ('select-pane','list-panes'): raise AssertionError('unexpected terminal action '+repr(args))
"#);
    fs::write(root.join("current-agent"),original_agent).unwrap();
    fs::write(root.join("initial-agent"),original_agent).unwrap();
    let history = root.join("home/.claude/projects/demo");
    fs::create_dir_all(&history).unwrap();
    fs::write(history.join(format!("{SESSION_A}.jsonl")),json!({"type":"user","message":{"content":[{"type":"text","text":"Finish the same requested change"}]}}).to_string()).unwrap();
    let cache = root.join("home/.cache/claude");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join(format!("statusline-{SESSION_A}.json")),json!({"model_id":"claude-opus-fixture","effort":"xhigh","updated":cockpit::util::now()}).to_string()).unwrap();
    let original_session = if original_agent == "claude" {SESSION_A} else {CODEX_SESSION};
    let original_model = if original_agent == "claude" {"claude-opus-fixture"} else {"gpt-fixture"};
    let original_effort = if original_agent == "claude" {"xhigh"} else {"high"};
    bench.world.borrow_mut().subscription_accounts.insert("runtime-10".into(),if original_agent == "claude" {"a".repeat(64)} else {"b".repeat(64)});
    if original_agent == "codex" {
        let directory = bench.registry.sessions[SESSION_A].cwd.clone();
        bench.registry.sessions.remove(SESSION_A);
        bench.registry.report_from(parse_session(&json!({"session":CODEX_SESSION,"native_thread":SESSION_A,"agent":"codex",
            "project":"project-a","cwd":directory,"state":"waiting"}).to_string()).unwrap(),10).unwrap();
    }
    let mut todos = cockpit::todo::TodoList::load();
    if assigned { assert!(todos.set_status("1d3fd318","doing",original_session)); }
    else {
        todos.items.clear();
        let directory = bench.registry.sessions[original_session].cwd.clone();
        let mut report = json!({"session":original_session,"agent":original_agent,"project":"project-a","cwd":directory,
            "state":"waiting","quota_stopped":true});
        if original_agent == "codex" {report["native_thread"] = json!(SESSION_A);}
        bench.registry.report_from(parse_session(&report.to_string()).unwrap(),10).unwrap();
    }
    todos.try_save().unwrap();
    let socket_folder = root.join("run/claude-team-host");
    fs::create_dir_all(&socket_folder).unwrap();
    let listener = UnixListener::bind(socket_folder.join("host.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let environment = [("PATH",format!("{}:{}",shims.display(),std::env::var("PATH").unwrap())),
        ("ROUNDTRIP_ROOT",root.display().to_string()),("COCKPIT_AGENTS_DIR",repository.join("config/bin/.local/bin/cockpit-agents").display().to_string())];
    run_host_command(&mut bench,&listener,&["subscription-sweep"],&environment);
    let fallback = read_routes(&cockpit::paths::subscription_routes()).unwrap().remove(0);
    assert!(matches!(fallback.phase,Phase::Active));
    assert_ne!(fallback.current.session,original_session);
    assert_ne!(fallback.current.selection.agent,original_agent);
    assert_eq!(cockpit::todo::TodoList::load().busy(&fallback.current.session),assigned);
    assert_eq!(fallback.is_session_task(),!assigned);
    assert!(!bench.world.borrow().sender_runtimes.contains_key(&10));
    assert!(bench.registry.request_from("subscription-gate",&json!({"generation":fallback.admitted_generation}).to_string(),40).is_ok());
    assert!(bench.registry.request_from("subscription-initial-gate",&json!({"generation":fallback.admitted_generation}).to_string(),40).is_ok());
    if !assigned {
        fs::write(root.join("unfinished"),"").unwrap();
        let directory = bench.registry.sessions[&fallback.current.session].cwd.clone();
        let mut report = json!({"session":fallback.current.session,"agent":fallback.current.selection.agent,
            "project":"project-a","cwd":directory,"state":"waiting","quota_stopped":true});
        if fallback.current.selection.agent == "codex" {report["native_thread"] = json!(SESSION_A);}
        bench.registry.report_from(parse_session(&report.to_string()).unwrap(),40).unwrap();
    }
    fs::write(root.join("recovered"),"").unwrap();
    run_host_command(&mut bench,&listener,&["subscription-sweep"],&environment);
    let returned = read_routes(&cockpit::paths::subscription_routes()).unwrap().remove(0);
    assert!(matches!(returned.phase,Phase::Active));
    assert_eq!(returned.current.session,original_session);
    assert_eq!(returned.current.selection.model,original_model);
    assert_eq!(returned.current.selection.variant.as_deref(),Some(original_effort));
    assert_eq!(returned.current.runtime,"runtime-return");
    assert_eq!(cockpit::todo::TodoList::load().busy(original_session),assigned);
    assert!(!bench.world.borrow().sender_runtimes.contains_key(&40));
    assert!(bench.registry.request_from("subscription-gate",&json!({"generation":returned.admitted_generation}).to_string(),11).is_ok());
    assert!(bench.registry.request_from("subscription-initial-gate",&json!({"generation":returned.admitted_generation}).to_string(),11).is_ok());
    }
    }
}
