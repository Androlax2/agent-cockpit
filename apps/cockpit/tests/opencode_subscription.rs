mod common;
use common::sandbox::*;
use serde_json::{Value, json};
use std::fs;

const ROOT: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";
const HARNESS: &str = r#"
const [module, scenario] = process.argv.slice(2)
const { setupSubscriptionRouting } = await import(module)
const config = JSON.parse(scenario)
const root = 'ses_3f2a9c0b1ffeQkTz8mWn4rJd2L'
let now = 1790000000000
Date.now = () => now
const original = config.original ?? {providerID:'openai', id:'gpt-6.1-sol', variant:'xhigh'}
let model = {...original}
let outcome = 'failed'
const calls = []
const messages = structuredClone(config.history ?? [])
const storage = new Map(Object.entries(config.saved ?? {}))
const timers = new Set()
globalThis.setInterval = (callback) => { timers.add(callback); return callback }
globalThis.clearInterval = (callback) => timers.delete(callback)
const hooks = new Map()
const retries = []
let blockedRequests = 0
let sourceUsed = config.sourceUsed ?? 100
let waiting = false
let release = () => {}
globalThis.fetch = async (url, input) => {
  if (input.headers.Authorization !== 'Bearer fixture-token') throw new Error('wrong auth transport')
  if (url === 'https://api.anthropic.com/api/oauth/usage') return new Response(JSON.stringify({
    five_hour:{utilization:config.targetUsed ?? 10,resets_at:'2099-01-01T00:00:00Z'},
    seven_day:{utilization:10,resets_at:'2099-01-01T00:00:00Z'},
    ...(config.malformedLimits ? {limits:{percent:100}} : {})
  }))
  if (url !== 'https://chatgpt.com/backend-api/wham/usage') throw new Error('unexpected quota URL')
  if (input.headers['ChatGPT-Account-Id'] !== (config.accountID??'fixture-account')) throw new Error('missing account binding')
  return new Response(JSON.stringify({account_id:config.wrongAccount ? 'another-account' : (config.accountID??'fixture-account'),
    rate_limit:{allowed:sourceUsed < 100,limit_reached:sourceUsed === 100,primary_window:{used_percent:sourceUsed,
      limit_window_seconds:18000,reset_at:4102444800}}}))
}
const method = provider => provider==='anthropic'?(config.targetMethod??'oauth'):(config.sourceMethod??'oauth')
const connection = (provider) => ({type:'credential',id:`${provider}-connection`,label:'Fixture account',
  ...(config.lightweightConnections ? {} : {method:method(provider)})})
const ctx = {
  storage:{get:async key=>storage.get(key),set:async(key,value)=>storage.set(key,structuredClone(value)),remove:async key=>storage.delete(key)},
  integration:{connection:{active:async provider=>connection(provider),resolve:async selected=>{
    const provider=selected.id.startsWith('anthropic')?'anthropic':'openai'
    return method(provider)==='oauth'?{type:'oauth',access:'fixture-token',expires:4102444800000,metadata:{accountID:config.accountID??'fixture-account'}}:{type:'key',key:'fixture-key'}
  }}},
  model:{list:async()=>[
    {providerID:'openai',id:'gpt-6.1-sol',enabled:true,capabilities:{tools:true},variants:[{id:'xhigh'}]},
    {providerID:'anthropic',id:'claude-opus-4-7',enabled:config.enabled??true,capabilities:{tools:config.tools??true},variants:[{id:'high'}]}
  ]},
  permission:{list:async()=>config.permission?[{id:'pending'}]:[]},
  session:{
    hook:async(name,callback)=>{hooks.set(name,callback);return {dispose:async()=>hooks.delete(name)}},
    get:async({sessionID})=>{
      if(sessionID===config.childID)return {id:sessionID,parentID:root,outcome:config.childBusy?undefined:'succeeded'}
      if(sessionID!==root)throw new Error('other session touched');return {id:root,model:{...model},outcome,agent:'build'}
    },
    wait:async({sessionID})=>{
      if(sessionID!==root && sessionID!==config.childID)throw new Error('other session touched')
      if(config.busy || sessionID===config.childID && config.childBusy){waiting=true;await new Promise(resolve=>release=resolve)}
    },
    context:async()=>messages,
    switchModel:async(input)=>{if(input.sessionID!==root)throw new Error('other session touched');calls.push({kind:'switch',...input});model={...input.model}},
    synthetic:async(input)=>{
      calls.push({kind:'continue',...input})
      messages.push({id:input.id,type:'synthetic',metadata:input.metadata})
      if(config.loseReceipt) throw new Error('fixture-token must never reach a log')
      return {id:input.id}
    }
  }
}
let router
try { router = await setupSubscriptionRouting(ctx,root) }
catch(error) {
  if(!config.expectInvalid)throw error
  console.log(JSON.stringify({caught:true,storage:[...storage.entries()]}))
  process.exit(0)
}
for (const step of config.steps ?? []) {
  if(step.used!==undefined){sourceUsed=step.used;now+=301000}
  if(step.targetUsed!==undefined){config.targetUsed=step.targetUsed;now+=301000}
  if(step.manual){model=step.manual;now+=301000}
  if(step.permission!==undefined) config.permission=step.permission
  if(step.accountID) config.accountID=step.accountID
  if(step.history) messages.splice(0,messages.length,...step.history)
  if(step.targetMethod) config.targetMethod=step.targetMethod
  if(step.sourceMethod) config.sourceMethod=step.sourceMethod
  if(step.retry){
    const event={sessionID:root,model:{...model},error:{status:429,type:'rate_limit'},attempt:2,decision:{retry:true,delay:1000}}
    await hooks.get('retry')(event)
    retries.push(event.decision)
  }
  if(step.userPrompt) await hooks.get('prompt')({sessionID:root,prompt:{text:'Explicit new request'}})
  if(step.request){
    const name=step.request.ws?'experimental.ws.handshake':'http.request'
    const headers={Authorization:step.request.wrongBearer?'Bearer fixture-key':'Bearer fixture-token',
      ...(step.request.apiKey?{'x-api-key':'fixture-key'}:{})}
    const url=step.request.ws?'wss://chatgpt.com/backend-api/codex/responses':'https://api.anthropic.com/v1/messages'
    try { await hooks.get(name)({sessionID:root,model:{...model},kind:step.request.kind??'primary',url,headers,request:new Request(url.replace('wss:','https:'),{headers})}) }
    catch(error){if(String(error).includes('is not a function'))throw error;blockedRequests++}
  }
  if(step.event){
    if(step.event.type==='session.execution.succeeded')outcome='succeeded'
    await router.handle(step.event)
  }
  if(step.startBusy){void router.handle({type:'session.execution.failed',data:{sessionID:root,error:{status:429}}});await new Promise(setImmediate)}
  if(step.release){config.busy=false;config.childBusy=false;release();await new Promise(setImmediate);await new Promise(setImmediate)}
  if(step.poll)for(const callback of [...timers])await callback()
  if(step.reload){await router.close();router=await setupSubscriptionRouting(ctx,root)}
}
await router.close()
console.log(JSON.stringify({calls,model,original,messages,retries,blockedRequests,waiting,timers:timers.size,hooks:hooks.size,storage:[...storage.entries()]}))
"#;

fn failed() -> Value {
    json!({"event":{"id":"evt_fixture_failure", "type":"session.execution.failed", "data":{"sessionID":ROOT, "error":{"status":429, "type":"rate_limit"}}}})
}

fn run(scenario: Value) -> Value {
    let sandbox = Sandbox::new();
    let harness = sandbox.root.join("subscription.mjs");
    fs::write(&harness, HARNESS).unwrap();
    let output = sandbox.command("node", &[]).arg(harness)
        .arg(repo().join("config/opencode/.config/opencode/lib/subscription-routing.ts"))
        .arg(scenario.to_string()).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stderr(&output).contains("fixture-token"));
    serde_json::from_str(stdout(&output).lines().last().unwrap()).unwrap()
}

#[test]
fn opencode_falls_back_and_restores_the_exact_original_model_without_another_prompt() {
    let result = run(json!({"steps":[failed(), {"event":{"type":"session.execution.succeeded", "data":{"sessionID":ROOT}}}, {"used":10,"poll":true}]}));
    let switches: Vec<&Value> = result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "switch").collect();
    assert_eq!(switches.len(), 2);
    assert_eq!(switches[0]["model"], json!({"providerID":"anthropic", "id":"claude-opus-4-7"}));
    assert_eq!(switches[1]["model"], result["original"]);
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "continue").count(), 1);
    assert_eq!(result["model"], result["original"]);
    assert_eq!(result["timers"], 0);
}

#[test]
fn opencode_can_fall_back_from_anthropic_and_return_to_its_original_variant() {
    let original = json!({"providerID":"anthropic","id":"claude-opus-4-7","variant":"high"});
    let result = run(json!({"original":original,"targetUsed":100,"sourceUsed":10,"steps":[failed(),
        {"event":{"type":"session.execution.succeeded","data":{"sessionID":ROOT}}},{"targetUsed":10,"poll":true}]}));
    let switches: Vec<&Value> = result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "switch").collect();
    assert_eq!(switches.len(), 2);
    assert_eq!(switches[0]["model"]["providerID"], "openai");
    assert_eq!(switches[1]["model"], original);
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "continue").count(), 1);
}

#[test]
fn opencode_rejects_api_key_unusable_and_wrong_account_alternatives() {
    for restriction in [json!({"targetMethod":"key"}), json!({"enabled":false}), json!({"tools":false}),
                        json!({"wrongAccount":true}), json!({"targetUsed":100}), json!({"malformedLimits":true})] {
        let mut scenario = restriction;
        scenario["steps"] = json!([failed()]);
        assert!(run(scenario)["calls"].as_array().unwrap().is_empty());
    }
}

#[test]
fn transient_rate_limits_do_not_switch_a_subscription_with_confirmed_capacity() {
    let result = run(json!({"sourceUsed":20,"steps":[failed()]}));
    assert!(result["calls"].as_array().unwrap().is_empty());
}

#[test]
fn plugin_connection_references_do_not_need_http_api_only_method_metadata() {
    let result = run(json!({"lightweightConnections":true,"steps":[failed()]}));
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "continue").count(), 1);
}

#[test]
fn running_native_shells_and_tools_prevent_model_switches() {
    for history in [json!([{"type":"shell","id":"msg_shell","status":"running"}]),
                    json!([{"type":"assistant","id":"msg_assistant","content":[{"type":"tool","name":"shell","state":{"status":"running"}}]}])] {
        let result = run(json!({"history":history,"steps":[failed()]}));
        assert!(result["calls"].as_array().unwrap().is_empty());
    }
}

#[test]
fn a_pending_form_survives_reload_and_blocks_switching_until_the_user_answers() {
    let result = run(json!({"steps":[{"event":{"type":"form.created","data":{"form":{"id":"form_fixture","sessionID":ROOT}}}},
        failed(),{"reload":true},{"poll":true}]}));
    assert!(result["calls"].as_array().unwrap().is_empty());
    let result = run(json!({"steps":[{"event":{"type":"form.created","data":{"form":{"id":"form_fixture","sessionID":ROOT}}}},
        failed(),{"event":{"type":"form.replied","data":{"id":"form_fixture","sessionID":ROOT,"answer":{}}}}, {"poll":true}]}));
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "continue").count(), 1);
}

#[test]
fn a_running_background_subagent_is_waited_for_without_changing_its_model() {
    let child = "ses_3f2a9c0b2000AbCdEfGhIjKlMn";
    let result = run(json!({"childID":child,"childBusy":true,"history":[{"type":"assistant","content":[{
        "type":"tool","name":"subagent","state":{"status":"completed","metadata":{"sessionID":child,"status":"running"}}}]}],
        "steps":[{"startBusy":true},{"release":true}]}));
    assert_eq!(result["waiting"], true);
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "continue").count(), 1);
}

#[test]
fn manual_model_changes_cancel_pending_automatic_return() {
    let manual = json!({"providerID":"anthropic", "id":"claude-opus-4-7", "variant":"high"});
    let result = run(json!({"steps":[failed(), {"manual":manual, "used":10,"poll":true}]}));
    assert_eq!(result["model"], manual);
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "switch").count(), 1);
}

#[test]
fn changing_the_account_inside_an_existing_connection_cancels_automatic_return() {
    let result = run(json!({"steps":[failed(),{"accountID":"replacement-account","used":10,"poll":true}]}));
    assert_eq!(result["model"]["providerID"], "anthropic");
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "switch").count(), 1);
}

#[test]
fn cached_capacity_for_a_previous_account_cannot_authorize_a_new_account() {
    let result = run(json!({"original":{"providerID":"anthropic","id":"claude-opus-4-7"},"targetUsed":100,"sourceUsed":100,
        "saved":{"subscription-quota/openai/openai-connection":{"fetched":1790000000000_i64,"matched":true,"account":"a".repeat(64),
            "usage":{"rate_limit":{"allowed":true,"primary_window":{"used_percent":10,"reset_at":4102444800_i64}}}}},"steps":[failed()]}));
    assert!(result["calls"].as_array().unwrap().is_empty());
}

#[test]
fn lost_admission_receipts_and_plugin_reload_do_not_duplicate_a_continuation() {
    let result = run(json!({"loseReceipt":true,"steps":[failed(), {"reload":true}, {"poll":true}]}));
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "continue").count(), 1);
}

#[test]
fn a_child_sessions_failure_does_not_change_the_root_model() {
    let result = run(json!({"steps":[{"event":{"type":"session.execution.failed", "data":{
        "sessionID":"ses_3f2a9c0b2000AbCdEfGhIjKlMn", "error":{"status":429}}}}]}));
    assert!(result["calls"].as_array().unwrap().is_empty());
}

#[test]
fn pending_permissions_block_a_fallback_without_answering_the_dialog() {
    let result = run(json!({"permission":true,"steps":[failed()]}));
    assert!(result["calls"].as_array().unwrap().is_empty());
}

#[test]
fn a_new_user_prompt_supersedes_a_blocked_subscription_continuation() {
    let result = run(json!({"permission":true,"steps":[failed(),{"permission":false,"userPrompt":true},{"poll":true}]}));
    assert!(result["calls"].as_array().unwrap().is_empty());
}

#[test]
fn invalid_saved_phases_are_preserved_instead_of_becoming_automatic_actions() {
    let owner = json!({"model":{"providerID":"openai", "id":"gpt-6.1-sol", "variant":"xhigh"}, "connection":"openai-connection"});
    let invalid = json!({"version":1,"original":owner,"current":owner,"phase":"force-api","pendingFailure":true});
    let key = format!("subscription-route/{ROOT}");
    let result = run(json!({"saved":{key.clone():invalid.clone()},"expectInvalid":true}));
    assert_eq!(result["caught"], true);
    assert_eq!(result["storage"], json!([[key,invalid]]));
}

#[test]
fn duplicate_failure_events_admit_only_one_continuation() {
    let result = run(json!({"steps":[failed(),failed(),{"reload":true},failed()]}));
    assert_eq!(result["calls"].as_array().unwrap().iter().filter(|call| call["kind"] == "continue").count(), 1);
}

#[test]
fn retry_hooks_stop_only_confirmed_subscription_exhaustion() {
    for (used, retry) in [(100, false), (20, true)] {
        let result = run(json!({"sourceUsed":used,"steps":[{"retry":true}]}));
        assert_eq!(result["retries"][0]["retry"], retry);
        assert!(result["calls"].as_array().unwrap().is_empty());
        assert_eq!(result["hooks"], 0);
    }
}

#[test]
fn automatic_continuations_reject_api_key_transport_but_manual_billing_changes_are_respected() {
    let result = run(json!({"steps":[failed(), {"request":{"apiKey":true}},
        {"targetMethod":"key", "userPrompt":true}, {"request":{"apiKey":true}}]}));
    assert_eq!(result["blockedRequests"], 1);
}

#[test]
fn websocket_requests_cannot_silently_switch_to_api_billing() {
    let result = run(json!({"targetUsed":100,"steps":[failed(), {"sourceMethod":"key","request":{"ws":true}}]}));
    assert_eq!(result["blockedRequests"], 1);
}

#[test]
fn auxiliary_requests_and_unexpected_bearer_tokens_cannot_bypass_subscription_billing_guards() {
    for request in [json!({"kind":"compaction","apiKey":true}),json!({"kind":"title","wrongBearer":true}),json!({"wrongBearer":true})] {
        let result = run(json!({"steps":[failed(),{"request":request}]}));
        assert_eq!(result["blockedRequests"], 1);
    }
}

#[test]
fn opencode_routing_matches_the_shared_provider_contract_cases() {
    let sandbox = Sandbox::new();
    let program = r#"
import {readFileSync} from 'node:fs'
const [module,fixture] = process.argv.slice(1)
const {subscriptionCapacity} = await import(module)
for(const test of JSON.parse(readFileSync(fixture,'utf8'))){
  const actual=subscriptionCapacity(test.provider,test.model,test.usage,1800000000000)
  if(actual!==test.expected)throw new Error(`${test.name}: expected ${test.expected}, got ${actual}`)
}
"#;
    let output = sandbox.command("node", &[]).args(["--input-type=module", "-e", program])
        .arg(repo().join("config/opencode/.config/opencode/lib/subscription-routing.ts"))
        .arg(repo().join("apps/cockpit/tests/fixtures/subscription-fallback/capacity-cases.json")).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
}
