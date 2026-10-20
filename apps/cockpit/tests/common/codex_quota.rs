#![allow(dead_code)]

use crate::common::sandbox::*;
use serde_json::json;
use std::fs;

pub fn sandbox() -> Sandbox {
    let mut sandbox = Sandbox::new();
    sandbox.env.insert("XDG_DATA_HOME".into(), sandbox.home.join(".local/share").display().to_string());
    sandbox.env.insert("CODEX_HOME".into(), sandbox.home.join("normal-codex").display().to_string());
    fs::write(sandbox.home.join("quota-response.json"), json!({
        "rateLimits": {"limitId":"codex", "primary":{"usedPercent":28,"windowDurationMins":300,"resetsAt":4102444800i64},
            "secondary":{"usedPercent":86,"windowDurationMins":10080,"resetsAt":4102444800i64}},
        "rateLimitsByLimitId": null
    }).to_string()).unwrap();
    fs::write(sandbox.home.join("account-response.json"), json!({"account":{"type":"chatgpt","email":"private@example.test","planType":"pro"},"requiresOpenaiAuth":true}).to_string()).unwrap();
    vendor(&sandbox, r#"
import json, os, sys
from pathlib import Path
home = Path.home()
assert Path(os.environ['CODEX_HOME']) == home / '.local/share/codex-quota'
assert sys.argv[1:] == ['app-server', '-c', 'cli_auth_credentials_store="file"']
def read():
    message = json.loads(sys.stdin.readline())
    with open(os.environ['SHIM_LOG'], 'a') as log:
        log.write(message['method'] + '\n')
    return message
def send(message):
    print(json.dumps(message), flush=True)
request = read()
assert request['method'] == 'initialize'
send({'id':request['id'], 'result':{'userAgent':'fake', 'platformFamily':'unix', 'platformOs':'linux'}})
assert read()['method'] == 'initialized'
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    with open(os.environ['SHIM_LOG'], 'a') as log:
        log.write(method + '\n')
    send({'method':'account/updated','params':{'authMode':'chatgpt','planType':'pro'}})
    if method == 'account/read':
        send({'id':request['id'], 'result':json.loads((home/'account-response.json').read_text())})
    elif method == 'account/rateLimits/read':
        send({'id':request['id'], 'result':json.loads((home/'quota-response.json').read_text())})
    elif method == 'account/login/start':
        assert request['params'] == {'type':'chatgpt'}
        send({'id':request['id'], 'result':{'type':'chatgpt','loginId':'test-login','authUrl':'https://auth.openai.com/test-login'}})
        if (home/'login-fails').exists():
            send({'method':'account/login/completed','params':{'loginId':'test-login','success':False,'error':'private-server-secret'}})
        else:
            send({'method':'account/login/completed','params':{'loginId':'other-login','success':False,'error':'ignore'}})
            send({'method':'account/login/completed','params':{'loginId':'test-login','success':True,'error':None}})
    else:
        raise AssertionError('Forbidden method: '+method)
"#);
    sandbox
}

pub fn vendor(sandbox: &Sandbox, body: &str) {
    write_executable(&sandbox.home.join(".local/npm/bin/codex"), &format!("#!/usr/bin/env python3\n{body}"));
}

pub fn client(sandbox: &Sandbox, arguments: &[&str]) -> std::process::Output {
    let helper = waybar_scripts().join("codex-quota.py");
    let mut full_arguments = vec![helper.to_str().unwrap()];
    full_arguments.extend_from_slice(arguments);
    sandbox.run("python3", &full_arguments, &[])
}
