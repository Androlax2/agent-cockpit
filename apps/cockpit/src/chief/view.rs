//! Plain escaped content and navigation for the approved F1–F7 Chief controls.
use crate::{app::Key, approval::escape_hiding};
use serde_json::{Value, json};

pub fn shown(value: &Value) -> String {
    escape_hiding(&serde_json::to_string_pretty(value).expect("JSON value serializes"), "", "")
}
fn text(value: &Value) -> String {
    value.as_str().map(|value|escape_hiding(value,"","")).unwrap_or_else(||shown(value))
}
fn host_flag(value: &Value) -> bool {value==true || value==1}
fn identity(record: &Value) -> String {
    format!("{}:{}",record["kind"],match record["kind"].as_str() {
        Some("worker"|"binding"|"team_member") => record["session"].to_string(),
        Some("work") => record["brief"]["work"].to_string(),
        Some("chief") => record["epoch"].to_string(),
        _ => record["id"].to_string(),
    })
}
fn critical(record: &Value) -> Value {
    match record["kind"].as_str() {
        // Availability is rechecked by the host, not consent to a role binding.
        Some("worker") => json!({"kind":"worker","session":record["session"],"role":record["role"]}),
        Some("work") => json!({"kind":"work","brief":record["brief"],"approval_hash":record["approval_hash"],"state":record["state"]}),
        _ => record.clone(),
    }
}
#[derive(Clone)]
enum Scope { Record(String), Task(Value), Session(Value), Chiefs }
#[derive(Clone)]
struct Guard { scope: Scope, records: Vec<Value> }
#[derive(Clone)]
enum Page {
    Root, Work, Pending, Approved, Workers, Decisions, Controls, Sessions,
    WorkActions(Value), Gates(Value), GateActions(Value,Value), Sources(Value,Value),
    WorkerActions(Value), Roles(Value), DecisionActions(Value), Recovery(Value),
}
#[derive(Clone)]
enum Intent { Resolve(Value), Handled(Value), Stop(Value), Attest(Value,Value) }
#[derive(Clone)]
enum Operation {
    Open(Page), Inspect(Value, Option<Page>), Review(Vec<Value>, Vec<Value>, String),
    Note(Intent), Summary, Unavailable(String),
}
struct Row { key: String, label: String, operation: Operation }
impl Row {
    fn new(key: &str, label: &str, operation: Operation) -> Self { Self {key:key.into(),label:label.into(),operation} }
    fn record(record: &Value, operation: Operation) -> Self {
        let label = match record["kind"].as_str() {
            Some("proposal"|"work") => format!("{} / {} · revision {} · {}",text(&record["brief"]["work"]["task"]["project"]),
                text(&record["brief"]["work"]["task"]["task_id"]),record["brief"]["work"]["revision"],text(&record["freshness"])),
            Some("worker") => format!("{} · {} · {} · {}",text(&record["session"]["agent"]),text(&record["session"]["project"]),
                text(&record["role"]),if record["session"]["agent"] != "opencode" {"monitor only"} else if host_flag(&record["ready"]) {"host reports available"} else {"unavailable"}),
            Some("decision") => format!("{} · {}",text(&record["decision_key"]),text(&record["question"])),
            Some("source") => format!("{} · {} · {}",text(&record["id"]),if record["can_import"]==true {"passing current source"} else {"unavailable"},text(&record["unavailable_reason"])),
            _ => identity(record),
        };
        Self {key:identity(record),label,operation}
    }
}
#[derive(Clone)]
enum Frame {
    Browse {page:Page, selected:Option<String>},
    Detail {record:Value, actions:Option<Page>, scroll:usize},
    Note {intent:Intent, note:String},
    Assertion {intent:Intent, note:String, checked:bool, scroll:usize},
    Review {actions:Vec<Value>, guards:Vec<Guard>, content:String, scroll:usize, confirm:bool, invalid:bool},
    Summary {scroll:usize},
    Results {content:String, scroll:usize},
}
pub struct ChiefView {
    pub snapshot: Value,
    frames: Vec<Frame>,
    selected_proposals: Vec<Value>,
    pub message: String,
    pub suspended: bool,
    pub page_step: usize,
    read_error: Option<String>,
}
pub enum Effect { None, Close, Submit(Vec<crate::chief::ChiefHostAction>), Summary }

impl ChiefView {
    pub fn new(snapshot: Value) -> Self {
        Self {snapshot,frames:vec![Frame::Browse {page:Page::Root,selected:Some("decisions".into())}],
            selected_proposals:Vec::new(),message:String::new(),suspended:false,page_step:1,read_error:None}
    }
    fn records(&self, kind: &str) -> Vec<Value> {
        self.snapshot["records"].as_array().into_iter().flatten().filter(|record|record["kind"]==kind).cloned().collect()
    }
    pub fn attention(snapshot: &Value) -> usize {
        snapshot["records"].as_array().into_iter().flatten().filter(|record|record["kind"]=="decision" && record["state"]=="open").count()
    }
    fn chiefs(&self) -> Vec<Value> { self.records("chief").into_iter().filter(|chief|chief["state"]!="disabled").collect() }
    pub fn status(&self) -> String {
        if self.read_error.is_some() {return "Unknown / host review unavailable — retained records are stale".into();}
        let chiefs=self.chiefs();
        match chiefs.as_slice() {
            [] => "Off".into(),
            [chief] if !host_flag(&chief["live_verified"]) => format!("Unavailable · retained {} grant",text(&chief["state"])),
            [chief] if self.unavailable_reason(chief).is_some() => format!("Unavailable · retained {} grant · {}",text(&chief["state"]),self.unavailable_reason(chief).expect("reason checked")),
            [chief] if chief["state"]=="paused" => "Paused · workers continue".into(),
            [chief] if chief["state"]=="active" => "Active".into(),
            _ => "Unknown chief state — host review required".into(),
        }
    }
    fn unavailable_reason(&self, chief: &Value) -> Option<String> {
        self.records("worker").iter().find(|worker|worker["session"]==chief["session"])
            .and_then(|worker|worker["blocked_by"].as_array()).and_then(|blockers|blockers.iter().find(|blocker|matches!(blocker.as_str(),Some(
                "native_admission_unverified_for_adapter"|"live_queued_admission_verification_required"|"fresh_native_availability_required"|"runtime_native_sample_unavailable"|"unavailable"))))
            .map(text)
    }
    fn current(&self, record: &Value) -> bool {
        self.snapshot["records"].as_array().into_iter().flatten().any(|current|identity(current)==identity(record) && critical(current)==critical(record))
    }
    fn scope_records(&self, scope: &Scope) -> Vec<Value> {
        self.snapshot["records"].as_array().into_iter().flatten().filter(|record|match scope {
            Scope::Record(key) => identity(record)==*key,
            Scope::Task(task) => matches!(record["kind"].as_str(),Some("work"|"proposal")) && record["brief"]["work"]["task"]==*task,
            Scope::Session(session) => matches!(record["kind"].as_str(),Some("worker"|"binding"|"team_member")) && record["session"]["session_id"]==session["session_id"],
            Scope::Chiefs => record["kind"]=="chief" && record["state"]!="disabled",
        }).map(critical).collect()
    }
    fn guards(&self, anchors: &[Value]) -> Vec<Guard> {
        let mut scopes=vec![Scope::Chiefs];
        for record in anchors {
            scopes.push(Scope::Record(identity(record)));
            if !record["brief"]["work"]["task"].is_null() {scopes.push(Scope::Task(record["brief"]["work"]["task"].clone()));}
            if !record["session"].is_null() {scopes.push(Scope::Session(record["session"].clone()));}
            for prerequisite in record["brief"]["prerequisites"].as_array().into_iter().flatten() {scopes.push(Scope::Task(prerequisite["task"].clone()));}
            if record["kind"]=="decision" {
                for assignment in self.records("assignment").iter().filter(|assignment|assignment["id"]==record["assignment"]) {scopes.push(Scope::Record(identity(assignment)));}
            }
        }
        scopes.into_iter().map(|scope|Guard {records:self.scope_records(&scope),scope}).collect()
    }
    pub fn refresh(&mut self, snapshot: Value) {
        self.snapshot=snapshot;
        self.read_error=None;
        self.selected_proposals=self.selected_proposals.iter().filter(|record|self.current(record)).cloned().collect();
        let selected: Vec<_> = self.frames.iter().map(|frame|match frame {
            Frame::Browse {page,selected} => Some(selected.as_ref().filter(|key|self.rows(page).iter().any(|row|row.key==**key)).cloned()),
            _ => None,
        }).collect();
        for (frame,selection) in self.frames.iter_mut().zip(selected) {
            if let (Frame::Browse {selected,..},Some(selection))=(frame,selection) {*selected=selection;}
        }
        let records=self.snapshot["records"].as_array().cloned().unwrap_or_default();
        for frame in &mut self.frames {
            if let Frame::Detail {record,actions,..}=frame {
                if let Some(current)=records.iter().find(|current|identity(current)==identity(record)) {
                    *record=current.clone();
                    *actions=match current["kind"].as_str() {
                        Some("worker")=>Some(Page::WorkerActions(current.clone())),Some("work")=>Some(Page::WorkActions(current.clone())),
                        Some("decision")=>Some(Page::DecisionActions(current.clone())),_=>actions.clone(),
                    };
                }
            }
        }
        if let Some(Frame::Review {guards,invalid,..})=self.frames.last() {
            if !invalid && guards.iter().any(|guard|self.scope_records(&guard.scope)!=guard.records) {
                self.invalidate("Reviewed records changed or disappeared. Cancel and review again.");
            }
        }
        self.prune_missing_details();
    }
    fn prune_missing_details(&mut self) {
        if matches!(self.frames.last(),Some(Frame::Review {..})) {return;}
        if let Some(index)=self.frames.iter().position(|frame|matches!(frame,Frame::Detail {record,..} if record["kind"]!="gate" &&
            !self.snapshot["records"].as_array().into_iter().flatten().any(|current|identity(current)==identity(record)))) {
            self.frames.truncate(index);
            self.message="Record disappeared. Move in the parent list to select another item.".into();
        }
    }
    pub fn suspend(&mut self) {
        self.suspended=true;
        if let Some(Frame::Review {confirm,..})=self.frames.last_mut() {*confirm=false;}
    }
    pub fn results(&mut self, content: String) {self.frames.push(Frame::Results {content,scroll:0});}
    pub fn fail(&mut self, message: String) {
        self.message=escape_hiding(&message,"","");
        if let Some(Frame::Review {invalid,..})=self.frames.last_mut() {*invalid=true;}
    }
    pub fn read_failed(&mut self, message: String) {
        self.read_error=Some(escape_hiding(&message,"",""));self.fail(message);
    }
    fn invalidate(&mut self, message: &str) {
        if let Some(Frame::Review {invalid,confirm,..})=self.frames.last_mut() {*invalid=true;*confirm=false;}
        self.message=message.into();
    }
    pub fn submission_pending(&self, key: &Key) -> bool {
        *key==Key::Char('y') && matches!(self.frames.last(),Some(Frame::Review {confirm:true,invalid:false,..}))
    }
    fn open(&mut self, page: Page) {
        let selected=self.rows(&page).first().map(|row|row.key.clone()); self.frames.push(Frame::Browse {page,selected});
    }
    fn standalone(&self, worker: &Value) -> bool {
        !self.records("team_member").iter().any(|member|member["session"]["session_id"]==worker["session"]["session_id"])
            && !self.chiefs().iter().any(|chief|chief["session"]["session_id"]==worker["session"]["session_id"])
    }
    fn action(action: &str, fields: Value) -> Value {
        let mut payload=fields; payload["action"]=json!(action); payload["request_id"]=json!(format!("chief-ui-{}",crate::util::token_hex(12))); payload
    }
    fn mutation(label: &str, action: Value, anchors: Vec<Value>, explanation: &str) -> Row {
        Row::new(label,label,Operation::Review(vec![action],anchors,explanation.into()))
    }
    fn rows(&self, page: &Page) -> Vec<Row> {
        use Operation::*;
        match page {
            Page::Root => vec![Row::new("decisions",&format!("Decisions · {}",Self::attention(&self.snapshot)),Open(Page::Decisions)),
                Row::new("work","Work",Open(Page::Work)),Row::new("workers","Workers and roles",Open(Page::Workers)),
                Row::new("controls","Chief controls",Open(Page::Controls)),Row::new("summary","Request summary",Summary)],
            Page::Work => vec![Row::new("pending","Awaiting approval",Open(Page::Pending)),Row::new("approved","Approved",Open(Page::Approved))],
            Page::Pending => self.records("proposal").into_iter().filter(|record|record["freshness"]=="pending").map(|record| {
                let mut row=Row::record(&record,Inspect(record.clone(),None));
                row.label=format!("[{}] Agent proposal · {}",if self.selected_proposals.contains(&record) {"x"} else {" "},row.label);row
            }).collect(),
            Page::Approved => self.records("work").into_iter().filter(|record|matches!(record["state"].as_str(),Some("approved"|"completed")))
                .map(|record|Row::record(&record,Inspect(record.clone(),Some(Page::WorkActions(record.clone()))))).collect(),
            Page::Workers => self.records("worker").into_iter().map(|record|Row::record(&record,Inspect(record.clone(),Some(Page::WorkerActions(record.clone()))))).collect(),
            Page::Decisions => self.records("decision").into_iter().filter(|record|record["state"]=="open")
                .map(|record|Row::record(&record,Inspect(record.clone(),Some(Page::DecisionActions(record.clone()))))).collect(),
            Page::Controls => {
                let chiefs=self.chiefs();
                if chiefs.is_empty() {return vec![Row::new("select","Select chief",Open(Page::Sessions))];}
                let mut rows=Vec::new();
                if let [chief]=chiefs.as_slice() {
                    if host_flag(&chief["live_verified"]) {
                        let (action,label,explanation)=if chief["state"]=="paused" {("resume","Resume chief","Resume revalidates the exact identity and grant.")}
                            else {("pause","Pause chief","New chief actions and automatic wake-ups stop. Accepted worker assignments continue.")};
                        rows.push(Self::mutation(label,Self::action(action,json!({"expected_epoch":chief["epoch"]})),vec![chief.clone()],explanation));
                    }
                    rows.push(Self::mutation("Disable chief",Self::action("disable",json!({"expected_epoch":chief["epoch"]})),vec![chief.clone()],
                        "Observation/dispatch access is revoked and unsent chief messages cancelled. Workers and teams continue independently. Completed effects are not undone."));
                }
                rows
            },
            Page::Sessions => self.records("worker").into_iter().filter(|worker|self.standalone(worker)
                && !self.records("binding").iter().any(|binding|binding["session"]["session_id"]==worker["session"]["session_id"] && binding["state"]=="bound")
                && !self.records("assignment").iter().any(|assignment|assignment["worker"]==worker["session"]["session_id"] && !matches!(assignment["state"].as_str(),Some("completed"|"cancelled"))))
                .map(|worker|Row::record(&worker,
                Review(vec![Self::action("enable",json!({"session":worker["session"]}))],vec![worker.clone()],
                    "Cross-project operational status and bounded briefs only. No extra file access, credentials, transcripts, raw approval payloads or permissions. Never starts an agent.".into()))).collect(),
            Page::WorkActions(work) => vec![Self::mutation("Revoke",Self::action("revoke_work",json!({"work":work["brief"]["work"]})),vec![work.clone()],
                "Pending assignments are cancelled. Running work receives a stop-at-safe-boundary notice. Completed effects are not undone. Uncertain ownership is not released."),
                Row::new("evidence","Evidence",Open(Page::Gates(work.clone())))],
            Page::Gates(work) => work["brief"]["gates"].as_array().into_iter().flatten().map(|gate|Row::new(&gate["kind"].to_string(),
                &format!("{} · source {}",text(&gate["kind"]),text(&gate["source"])),Inspect(json!({"kind":"gate","work":work["brief"]["work"],"gate":gate,"approval_hash":work["approval_hash"]}),
                    Some(Page::GateActions(work.clone(),gate.clone()))))).collect(),
            Page::GateActions(work,gate) => vec![Row::new("import","Import recorded evidence",Open(Page::Sources(work.clone(),gate.clone()))),
                Row::new("attest",if gate["source"]=="host_attestation" {"Attest evidence"} else {"Attest evidence — unavailable: not an approved host_attestation source"},
                    if gate["source"]=="host_attestation" {Note(Intent::Attest(work.clone(),gate.clone()))} else {Unavailable("Only an already-approved host_attestation gate can be attested.".into())})],
            Page::Sources(work,gate) => self.records("source").into_iter().filter(|source|source["evidence"]["work"]==work["brief"]["work"] && source["evidence"]["kind"]==gate["kind"])
                .map(|source|Row::record(&source,if source["can_import"]==true {Review(vec![Self::action("import_evidence",json!({"work":work["brief"]["work"],"gate":gate["kind"],"source_id":source["id"]}))],
                    vec![work.clone(),source.clone()],"Import this exact protected source. Host resolves its result, never a caller path. Worker prose is not proof.".into())}
                    else {Inspect(source.clone(),None)})).collect(),
            Page::WorkerActions(worker) => vec![Row::new("bind","Bind role",if self.standalone(worker) {Open(Page::Roles(worker.clone()))}
                else {Unavailable("Bind role unavailable: existing team mapping or selected chief. Team mappings remain unchanged.".into())})],
            Page::Roles(worker) => {
                if !self.standalone(worker) {return vec![Row::new("unavailable","Bind role — unavailable: team mapping or selected chief",Unavailable("Existing team mappings remain unchanged; the chief cannot be a product worker.".into()))];}
                let binding=self.records("binding").into_iter().find(|binding|binding["session"]["session_id"]==worker["session"]["session_id"] && binding["state"]=="bound");
                ["designer","developer","qa"].into_iter().map(|role| {
                    let mut anchors=vec![worker.clone()];if let Some(binding)=&binding {anchors.push(binding.clone());}
                    Self::mutation(role,Self::action("bind_worker",json!({"session":worker["session"],"role":role,"expected_epoch":binding.as_ref().map(|binding|binding["epoch"].clone())})),anchors,
                        "Bind standalone role only. Does not grant readiness, admission, access, live verification or dispatch eligibility. Host-native live verification is required. Claude/Codex remain monitor-only.")
                }).collect()
            },
            Page::DecisionActions(decision) => {
                let mut rows=vec![Row::new("resolve","Resolve with note",Note(Intent::Resolve(decision.clone())))];
                if let Some(assignment)=self.records("assignment").into_iter().find(|assignment|assignment["id"]==decision["assignment"] && matches!(assignment["state"].as_str(),Some("uncertain"|"blocked"))) {
                    rows.push(Row::new("recovery","Recovery",Open(Page::Recovery(assignment))));
                }
                if let Some(delivery)=self.records("inbox").into_iter().find(|delivery|decision["scope_key"]==format!("host-delivery-{}",text(&delivery["id"])) && matches!(delivery["state"].as_str(),Some("uncertain"|"failed"))) {
                    rows.push(Row::new("recovery","Recovery",Open(Page::Recovery(delivery))));
                }
                if let Some(launch)=self.records("legacy_launch").into_iter().find(|launch|decision["scope_key"]==format!("host-launch-{}",text(&launch["id"])) && launch["state"]!="released") {
                    rows.push(Row::new("recovery","Recovery",Open(Page::Recovery(launch))));
                }
                rows
            },
            Page::Recovery(assignment) => vec![Row::new("handled","Confirm handled",Note(Intent::Handled(assignment.clone()))),
                Row::new("stop","Confirm safe stop and cancel",Note(Intent::Stop(assignment.clone())))],
        }
    }
    fn anchors(intent: &Intent) -> Vec<Value> {match intent {
        Intent::Resolve(record)|Intent::Handled(record)|Intent::Stop(record)=>vec![record.clone()],
        Intent::Attest(work,_)=>vec![work.clone()],
    }}
    fn begin_review(&mut self, actions: Vec<Value>, anchors: Vec<Value>, explanation: String) {
        if anchors.iter().any(|record|!self.current(record)) {self.message="Record changed or disappeared. Return and inspect again.".into();return;}
        let mut content=explanation;
        for record in &anchors {content.push_str("\n\n");content.push_str(&shown(record));}
        content.push_str("\n\nExact host requests:\n"); content.push_str(&shown(&json!(actions)));
        self.frames.push(Frame::Review {guards:self.guards(&anchors),actions,content,scroll:0,confirm:false,invalid:false});
    }
    fn finish_note(&mut self, intent: Intent, note: String) {
        let action=match intent.clone() {
            Intent::Resolve(decision)=>Self::action("resolve",json!({"decision":decision["id"],"expected_revision":decision["revision"],"resolution":note})),
            Intent::Handled(target)=>Self::recovery_action(&target,json!({"kind":"handled","note":note})),
            Intent::Stop(target)=>Self::recovery_action(&target,json!({"kind":"safe_stop","note":note,"confirmed_stopped":true})),
            Intent::Attest(work,gate)=>Self::action("attest_evidence",json!({"evidence":{"id":format!("attest-{}",crate::util::token_hex(12)),
                "work":work["brief"]["work"],"kind":gate["kind"],"commit":gate["expected"]["commit"],"source":"host_attestation","result":{"passed":true,"note":note}}})),
        };
        let explanation=match intent {Intent::Resolve(_)=>"Record a resolution note only. Grants no authority, waives no evidence and does not resolve a permission prompt.",
            Intent::Handled(_)=>"Record handled; retain ownership. This is not completion. Never blindly resend or reassign.",
            Intent::Stop(_)=>"You explicitly confirm the exact worker can no longer execute this assignment. Host validation remains authoritative; unknown/unresponsive ownership stays blocked.",
            Intent::Attest(_,_)=>"Theo’s attestation—not independently observed execution. Explicit assertion: I attest this exact approved gate passed."};
        self.begin_review(vec![action],Self::anchors(&intent),explanation.into());
    }
    fn recovery_action(target: &Value, evidence: Value) -> Value {
        if target["kind"]=="inbox" {Self::action("reconcile_delivery",json!({"message":target["id"],"expected_revision":target["revision"],"evidence":evidence}))}
        else if target["kind"]=="legacy_launch" {Self::action("reconcile_launch",json!({"token":target["id"],"expected_revision":target["revision"],"evidence":evidence}))}
        else {Self::action("reconcile",json!({"assignment":target["id"],"expected_revision":target["revision"],"evidence":evidence}))}
    }
    pub fn handle(&mut self, key: Key) -> Effect {
        self.message.clear();
        let Some(frame)=self.frames.last().cloned() else {return Effect::Close};
        match frame {
            Frame::Browse {page,selected} => {
                let rows=self.rows(&page);let index=selected.as_ref().and_then(|key|rows.iter().position(|row|row.key==*key));
                match key {
                    Key::Down|Key::Char('j')|Key::Up|Key::Char('k') => {
                        let down=matches!(key,Key::Down|Key::Char('j'));
                        let next=index.map(|index|if down {(index+1).min(rows.len().saturating_sub(1))} else {index.saturating_sub(1)}).unwrap_or(0);
                        if let Some(Frame::Browse {selected,..})=self.frames.last_mut() {*selected=rows.get(next).map(|row|row.key.clone());}
                    },
                    Key::Char(' ') if matches!(page,Page::Pending) => if let Some(index)=index {
                        if let Operation::Inspect(record,_)=&rows[index].operation {
                            if self.selected_proposals.contains(record) {self.selected_proposals.retain(|selected|selected!=record);} else {self.selected_proposals.push(record.clone());}
                        }
                    },
                    Key::Char('a') if matches!(page,Page::Pending) => {
                        let anchors=self.selected_proposals.clone();
                        if anchors.is_empty() {self.message="Select exact proposals first. Empty selection sends nothing.".into();}
                        else {
                            let actions=anchors.iter().map(|record|Self::action("approve_work",json!({"brief":record["brief"],"approval_hash":record["approval_hash"],
                                "expected_revision":record["brief"]["work"]["revision"].as_i64().unwrap_or(0)-1}))).collect();
                            let explanation=format!("Approve these {} exact briefs—not future revisions. Permits bounded work only; no additional tool, install or Git permissions. Each request is independent, not an atomic batch.",anchors.len());
                            self.begin_review(actions,anchors,explanation);
                        }
                    },
                    Key::Enter => if let Some(index)=index {match rows[index].operation.clone() {
                        Operation::Open(page)=>self.open(page),
                        Operation::Inspect(record,actions)=>self.frames.push(Frame::Detail {record,actions,scroll:0}),
                        Operation::Review(actions,anchors,explanation)=>self.begin_review(actions,anchors,explanation),
                        Operation::Note(intent)=>self.frames.push(Frame::Note {intent,note:String::new()}),
                        Operation::Unavailable(reason)=>self.message=reason,
                        Operation::Summary=>{self.frames.push(Frame::Summary {scroll:0});return Effect::Summary;},
                    }},
                    Key::Esc=>{self.frames.pop();},
                    _=>{},
                }
            },
            Frame::Detail {record,actions,..} => match key {
                Key::Enter=>if let Some(page)=actions {if record["kind"]=="gate" || self.current(&record) {self.open(page);} else {self.message="Record changed or disappeared. Return and inspect again.".into();}},
                Key::Esc=>{self.frames.pop();},
                _=>self.scroll(&key),
            },
            Frame::Note {intent,mut note} => match key {
                Key::Char(character) if !character.is_control()=>{if note.chars().count()<1000 {note.push(character);} if let Some(Frame::Note {note:current,..})=self.frames.last_mut() {*current=note;}},
                Key::Paste(paste)=>{for character in paste.chars().filter(|character|!character.is_control()) {if note.chars().count()<1000 {note.push(character);}} if let Some(Frame::Note {note:current,..})=self.frames.last_mut() {*current=note;}},
                Key::Backspace=>{note.pop();if let Some(Frame::Note {note:current,..})=self.frames.last_mut() {*current=note;}},
                Key::Enter if !note.trim().is_empty()=> {
                    self.frames.pop();
                    if matches!(intent,Intent::Stop(_)|Intent::Attest(_,_)) {self.frames.push(Frame::Assertion {intent,note,checked:false,scroll:0});}
                    else {self.finish_note(intent,note);}
                },
                Key::Esc=>{self.frames.pop();},
                _=>{},
            },
            Frame::Assertion {intent,note,checked,..}=>match key {
                Key::Char(' ')=>if let Some(Frame::Assertion {checked,..})=self.frames.last_mut() {*checked=!*checked;},
                Key::Enter if checked=>{self.frames.pop();self.finish_note(intent,note);},
                Key::Esc=>{self.frames.pop();},
                _=>self.scroll(&key),
            },
            Frame::Review {actions,confirm,invalid,..}=>match key {
                Key::Enter if !confirm && !invalid=>if let Some(Frame::Review {confirm,..})=self.frames.last_mut() {*confirm=true;},
                Key::Char('y') if confirm && !invalid=> {
                    let parsed=actions.into_iter().map(|action|serde_json::from_value::<crate::chief::ChiefHostAction>(action).and_then(|action| {
                        action.validate().map_err(serde::de::Error::custom)?;Ok(action)
                    })).collect::<Result<Vec<_>,_>>();
                    match parsed {Ok(actions)=>{self.frames.pop();self.selected_proposals.clear();return Effect::Submit(actions);},Err(error)=>self.fail(format!("Cannot submit: {error}"))}
                },
                Key::Esc|Key::Char('n') if confirm=>if let Some(Frame::Review {confirm,..})=self.frames.last_mut() {*confirm=false;},
                Key::Esc=>{self.frames.pop();},
                _=>self.scroll(&key),
            },
            Frame::Summary {..}|Frame::Results {..}=>if key==Key::Esc {self.frames.pop();} else {self.scroll(&key);},
        }
        self.prune_missing_details();
        if self.frames.is_empty() {Effect::Close} else {Effect::None}
    }
    fn scroll(&mut self, key: &Key) {
        let scroll=match self.frames.last_mut() {Some(Frame::Detail {scroll,..}|Frame::Review {scroll,..}|Frame::Assertion {scroll,..}|Frame::Summary {scroll}|Frame::Results {scroll,..})=>scroll,_=>return};
        match key {Key::PageDown=>*scroll=scroll.saturating_add(self.page_step),Key::PageUp=>*scroll=scroll.saturating_sub(self.page_step),_=>{}}
    }
    pub fn clamp_scroll(&mut self, maximum: usize) -> usize {
        match self.frames.last_mut() {Some(Frame::Detail {scroll,..}|Frame::Review {scroll,..}|Frame::Assertion {scroll,..}|Frame::Summary {scroll}|Frame::Results {scroll,..})=>{*scroll=(*scroll).min(maximum);*scroll},_=>0}
    }
    pub fn is_list(&self) -> bool {matches!(self.frames.last(),Some(Frame::Browse {..}))}
    pub fn list_window(&self, height: usize) -> (String,Vec<String>) {
        let Some(Frame::Browse {page,selected})=self.frames.last() else {return (String::new(),Vec::new())};
        let caption=match page {
            Page::Pending=>format!("Chief · Selected: {}",self.selected_proposals.len()),
            Page::Approved=>"Chief · Approved work".into(),Page::Work=>"Chief · Work".into(),
            Page::Workers=>"Chief · Workers and roles".into(),Page::Controls=>format!("Chief controls · {}",self.status()),
            Page::Sources(_,_)=>"Chief · Protected sources".into(),Page::Gates(_)=>"Chief · Required gates".into(),
            _=>format!("Chief · {}",self.status()),
        };
        let mut lines=Vec::new();
        if let Some(error)=&self.read_error {lines.push(format!("Stale · {error}"));}
        if !self.message.is_empty() {lines.push(self.message.clone());}
        if !self.snapshot["coordination_error"].is_null() {lines.push(format!("Host failure: {}",shown(&self.snapshot["coordination_error"])));}
        if matches!(page,Page::Gates(_)) {lines.push("Required gates: missing gates never imply not applicable.".into());}
        let available=height.saturating_sub(lines.len()).max(1);
        let rows=self.rows(page);
        if rows.is_empty() {lines.push("Nothing available. Missing records never imply readiness.".into());}
        let position=selected.as_ref().and_then(|key|rows.iter().position(|row|row.key==*key)).unwrap_or(0);
        for row in rows.iter().skip(position.saturating_sub(available.saturating_sub(1))).take(available) {
            lines.push(format!("{} {}",if selected.as_ref()==Some(&row.key) {">"} else {" "},row.label));
        }
        (caption,lines)
    }
    pub fn detail(&self) -> Vec<String> {
        let mut lines=vec![format!("Chief · {}",self.status())];
        if let Some(error)=&self.read_error {lines.push(format!("Host read failure: {error}"));}
        if !self.snapshot["coordination_error"].is_null() {lines.push(format!("Host failure: {}",shown(&self.snapshot["coordination_error"])));}
        if !self.message.is_empty() {lines.push(self.message.clone());}
        match self.frames.last() {
            Some(Frame::Browse {page,selected})=> {
                lines.push(match page {Page::Pending=>format!("Awaiting approval · Selected: {} exact brief revisions",self.selected_proposals.len()),Page::Approved=>"Approved work".into(),
                    Page::Controls=>"Chief controls".into(),Page::Workers=>"Workers and roles · role binding does not grant host-native verification".into(),Page::Sources(_,_)=>"Protected source records".into(),
                    Page::Gates(_)=>"Required gates — missing gates never imply not applicable".into(),_=>String::new()});
                let rows=self.rows(page);if rows.is_empty() {lines.push("Nothing available. Missing records do not imply readiness or approval.".into());}
                let selected_index=selected.as_ref().and_then(|key|rows.iter().position(|row|row.key==*key));
                let first=selected_index.unwrap_or(0).saturating_sub(4);
                for row in rows.iter().skip(first) {lines.push(format!("{} {}",if selected.as_ref()==Some(&row.key) {">"} else {" "},row.label));}
            },
            Some(Frame::Detail {record,..})=> {
                if record["kind"]=="worker" {
                    lines.push("Availability is daemon-derived; runtime reports are not independently proven. Unknown/stale means unavailable. Claude/Codex monitor-only. Binding grants no admission; host-native live verification is still required.".into());
                    for related in self.records("binding").into_iter().chain(self.records("team_member")) .filter(|related|related["session"]["session_id"]==record["session"]["session_id"]) {lines.push(shown(&related));}
                    let age=record["observed_at_ms"].as_i64().map(|observed|((crate::util::now()*1000.0) as i64).saturating_sub(observed));
                    lines.push(format!("Observation age (ms): {}",shown(&json!(age))));
                    let admissions:Vec<_>=self.records("admission").into_iter().filter(|admission|admission["session"]==record["session"]).collect();
                    lines.push(format!("Host-native admission verification records (empty means not verified): {}",shown(&json!(admissions))));
                }
                if record["kind"]=="decision" {
                    lines.push("Source: protected host decision; chief question/recommendation are untrusted narrative, not evidence or permission.".into());
                    for assignment in self.records("assignment").into_iter().filter(|assignment|assignment["id"]==record["assignment"]) {lines.push(format!("Protected assignment source: {}",shown(&assignment)));}
                }
                lines.push(shown(record));
                if record["kind"]=="gate" {
                    lines.push("Accepted evidence: QA is a role-bound attestation, not independently proven command execution. Worker prose is unverified. Apply evidence records reviewed work and installation outcome.".into());
                    let evidence:Vec<_>=self.records("evidence").into_iter().filter(|evidence|evidence["work"]==record["work"] && evidence["gate"]==record["gate"]["kind"]).collect();
                    if evidence.is_empty() {lines.push("No accepted evidence is recorded for this exact gate/revision.".into());}
                    for evidence in evidence {lines.push(shown(&evidence));}
                }
            },
            Some(Frame::Note {note,..})=>lines.push(format!("Nonempty note (not consent): {}",escape_hiding(note,"",""))),
            Some(Frame::Assertion {intent,note,checked,..})=> {
                for anchor in Self::anchors(intent) {lines.push(shown(&anchor));}
                lines.push(format!("Note: {}",escape_hiding(note,"","")));
                lines.push(format!("[{}] {}",if *checked {"x"} else {" "},if matches!(intent,Intent::Stop(target) if target["kind"]=="legacy_launch") {"I confirm this exact launch can no longer execute; its worker identity may be unknown."} else if matches!(intent,Intent::Stop(target) if target["kind"]=="inbox") {"I confirm this exact delivery and its recipient work can no longer execute."} else if matches!(intent,Intent::Stop(_)) {"I confirm this worker can no longer execute this assignment."} else {"I attest this exact gate passed. Theo’s attestation—not independently observed execution."}));
            },
            Some(Frame::Review {content,confirm,invalid,..})=> {
                lines.push(if *invalid {"Review invalid: records changed or unavailable. Fresh review required."} else if *confirm {"CONFIRM these exact reviewed actions? y/n"} else {"Frozen review — Enter opens separate confirmation; y does nothing here."}.into());lines.push(content.clone());
            },
            Some(Frame::Summary {..})=> {
                lines.push("Requested summary only. Chief narrative/worker claims are unverified; accepted evidence references retain their provenance.".into());
                let summary=self.records("summary");if summary.is_empty() {lines.push("Summary unavailable — host has no result.".into());} else {for record in summary {lines.push(shown(&record));}}
            },Some(Frame::Results {content,..})=>lines.push(content.clone()),None=>{},
        }
        lines
    }
    pub fn footer(&self) -> &str {match self.frames.last() {
        Some(Frame::Browse {page:Page::Pending,..})=>"Space select · Enter inspect · a review selected · Esc back",
        Some(Frame::Browse {..})=>"j/k ↑↓ move · Enter open · Esc back",
        Some(Frame::Note {..})=>"Enter accepts note, not action · Esc cancel",
        Some(Frame::Assertion {..})=>"Space toggle assertion · Enter review · PgUp/PgDn scroll · Esc cancel",
        Some(Frame::Review {confirm:true,..})=>"y submit exact actions · n/Esc return to frozen review",
        Some(Frame::Review {..})=>"PgUp/PgDn scroll full review · Enter confirm · Esc cancel",
        Some(Frame::Summary {..})=>"PgUp/PgDn scroll · Esc back · Enter never regenerates",
        Some(Frame::Results {..})=>"PgUp/PgDn scroll every host result · Esc back · no retry",
        _=>"PgUp/PgDn scroll · Enter actions (if available) · Esc back",
    }}
    pub fn compact_footer(&self) -> &str {match self.frames.last() {
        Some(Frame::Review {confirm:true,..})=>"y submit · n back\nEsc cancel",
        Some(Frame::Review {..})=>"PgUp/Dn Enter next\nEsc cancel",
        _=>self.footer(),
    }}
}
