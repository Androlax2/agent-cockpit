//! Plain escaped content and navigation for the Chief controls.
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
        Some("worker"|"team_member") => record["session"].to_string(),
        Some("chief") => record["epoch"].to_string(),
        _ => record["id"].to_string(),
    })
}
fn critical(record: &Value) -> Value {
    match record["kind"].as_str() {
        // Availability is rechecked by the host, not consent to a role binding.
        Some("worker") => json!({"kind":"worker","session":record["session"],"role":record["role"]}),
        _ => record.clone(),
    }
}
#[derive(Clone)]
enum Scope { Record(String), Session(Value), Chiefs }
#[derive(Clone)]
struct Guard { scope: Scope, records: Vec<Value> }
#[derive(Clone)]
enum Page {
    Root, Workers, Decisions, Controls, Sessions, DecisionActions(Value), Recovery(Value),
}
#[derive(Clone)]
enum Intent { Resolve(Value), Handled(Value), Stop(Value) }
#[derive(Clone)]
enum Operation {
    Open(Page), Inspect(Value, Option<Page>), Review(Vec<Value>, Vec<Value>, String),
    Note(Intent), Summary,
}
struct Row { key: String, label: String, operation: Operation }
impl Row {
    fn new(key: &str, label: &str, operation: Operation) -> Self { Self {key:key.into(),label:label.into(),operation} }
    fn record(record: &Value, operation: Operation) -> Self {
        let label = match record["kind"].as_str() {
            Some("worker") => format!("{} · {} · {} · {}",text(&record["session"]["agent"]),text(&record["session"]["project"]),
                text(&record["role"]),if record["session"]["agent"] != "opencode" {"monitor only"} else if host_flag(&record["ready"]) {"host reports available"} else {"unavailable"}),
            Some("decision") => format!("{} · {}",text(&record["decision_key"]),text(&record["question"])),
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
    pub message: String,
    pub suspended: bool,
    pub page_step: usize,
    read_error: Option<String>,
}
pub enum Effect { None, Close, Submit(Vec<crate::chief::ChiefHostAction>), Summary }

impl ChiefView {
    pub fn new(snapshot: Value) -> Self {
        Self {snapshot,frames:vec![Frame::Browse {page:Page::Root,selected:Some("decisions".into())}],
            message:String::new(),suspended:false,page_step:1,read_error:None}
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
            Scope::Session(session) => matches!(record["kind"].as_str(),Some("worker"|"team_member")) && record["session"]["session_id"]==session["session_id"],
            Scope::Chiefs => record["kind"]=="chief" && record["state"]!="disabled",
        }).map(critical).collect()
    }
    fn guards(&self, anchors: &[Value]) -> Vec<Guard> {
        let mut scopes=vec![Scope::Chiefs];
        for record in anchors {
            scopes.push(Scope::Record(identity(record)));
            if !record["session"].is_null() {scopes.push(Scope::Session(record["session"].clone()));}
        }
        scopes.into_iter().map(|scope|Guard {records:self.scope_records(&scope),scope}).collect()
    }
    pub fn refresh(&mut self, snapshot: Value) {
        self.snapshot=snapshot;
        self.read_error=None;
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
        if let Some(index)=self.frames.iter().position(|frame|matches!(frame,Frame::Detail {record,..} if !self.snapshot["records"].as_array().into_iter().flatten().any(|current|identity(current)==identity(record)))) {
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
                Row::new("workers","Workers",Open(Page::Workers)),
                Row::new("controls","Chief controls",Open(Page::Controls)),Row::new("summary","Request summary",Summary)],
            Page::Workers => self.records("worker").into_iter().map(|record|Row::record(&record,Inspect(record.clone(),None))).collect(),
            Page::Decisions => self.records("decision").into_iter().filter(|record|record["state"]=="open")
                .map(|record|Row::record(&record,Inspect(record.clone(),Some(Page::DecisionActions(record.clone()))))).collect(),
            Page::Controls => {
                let chiefs=self.chiefs();
                if chiefs.is_empty() {return vec![Row::new("select","Select chief",Open(Page::Sessions))];}
                let mut rows=Vec::new();
                if let [chief]=chiefs.as_slice() {
                    if host_flag(&chief["live_verified"]) {
                        let (action,label,explanation)=if chief["state"]=="paused" {("resume","Resume chief","Resume revalidates the exact identity and grant.")}
                            else {("pause","Pause chief","New chief actions and automatic wake-ups stop.")};
                        rows.push(Self::mutation(label,Self::action(action,json!({"expected_epoch":chief["epoch"]})),vec![chief.clone()],explanation));
                    }
                    rows.push(Self::mutation("Disable chief",Self::action("disable",json!({"expected_epoch":chief["epoch"]})),vec![chief.clone()],
                        "Chief access is revoked and unsent chief messages cancelled. Workers and teams continue independently. Completed effects are not undone."));
                }
                rows
            },
            Page::Sessions => self.records("worker").into_iter().filter(|worker|self.standalone(worker))
                .map(|worker|Row::record(&worker,
                Review(vec![Self::action("enable",json!({"session":worker["session"]}))],vec![worker.clone()],
                    "Cross-project operational status and bounded briefs only. No extra file access, credentials, transcripts, raw approval payloads or permissions. Never starts an agent.".into()))).collect(),
            Page::DecisionActions(decision) => {
                let mut rows=vec![Row::new("resolve","Resolve with note",Note(Intent::Resolve(decision.clone())))];
                if let Some(delivery)=self.records("inbox").into_iter().find(|delivery|decision["scope_key"]==format!("host-delivery-{}",text(&delivery["id"])) && matches!(delivery["state"].as_str(),Some("uncertain"|"failed"))) {
                    rows.push(Row::new("recovery","Recovery",Open(Page::Recovery(delivery))));
                }
                if let Some(launch)=self.records("legacy_launch").into_iter().find(|launch|decision["scope_key"]==format!("host-launch-{}",text(&launch["id"])) && launch["state"]!="released") {
                    rows.push(Row::new("recovery","Recovery",Open(Page::Recovery(launch))));
                }
                rows
            },
            Page::Recovery(target) => vec![Row::new("handled","Confirm handled",Note(Intent::Handled(target.clone()))),
                Row::new("stop","Confirm safe stop and cancel",Note(Intent::Stop(target.clone())))],
        }
    }
    fn anchors(intent: &Intent) -> Vec<Value> {match intent {
        Intent::Resolve(record)|Intent::Handled(record)|Intent::Stop(record)=>vec![record.clone()],
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
        };
        let explanation=match intent {Intent::Resolve(_)=>"Record a resolution note only. Grants no authority and does not resolve a permission prompt.",
            Intent::Handled(_)=>"Record handled; retain ownership. This is not completion. Never blindly resend or reassign.",
            Intent::Stop(_)=>"You explicitly confirm the exact launch or delivery recipient can no longer execute. Host validation remains authoritative; unknown/unresponsive ownership stays blocked."};
        self.begin_review(vec![action],Self::anchors(&intent),explanation.into());
    }
    fn recovery_action(target: &Value, evidence: Value) -> Value {
        if target["kind"]=="inbox" {Self::action("reconcile_delivery",json!({"message":target["id"],"expected_revision":target["revision"],"evidence":evidence}))}
        else {Self::action("reconcile_launch",json!({"token":target["id"],"expected_revision":target["revision"],"evidence":evidence}))}
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
                    Key::Enter => if let Some(index)=index {match rows[index].operation.clone() {
                        Operation::Open(page)=>self.open(page),
                        Operation::Inspect(record,actions)=>self.frames.push(Frame::Detail {record,actions,scroll:0}),
                        Operation::Review(actions,anchors,explanation)=>self.begin_review(actions,anchors,explanation),
                        Operation::Note(intent)=>self.frames.push(Frame::Note {intent,note:String::new()}),
                        Operation::Summary=>{self.frames.push(Frame::Summary {scroll:0});return Effect::Summary;},
                    }},
                    Key::Esc=>{self.frames.pop();},
                    _=>{},
                }
            },
            Frame::Detail {record,actions,..} => match key {
                Key::Enter=>if let Some(page)=actions {if self.current(&record) {self.open(page);} else {self.message="Record changed or disappeared. Return and inspect again.".into();}},
                Key::Esc=>{self.frames.pop();},
                _=>self.scroll(&key),
            },
            Frame::Note {intent,mut note} => match key {
                Key::Char(character) if !character.is_control()=>{if note.chars().count()<1000 {note.push(character);} if let Some(Frame::Note {note:current,..})=self.frames.last_mut() {*current=note;}},
                Key::Paste(paste)=>{for character in paste.chars().filter(|character|!character.is_control()) {if note.chars().count()<1000 {note.push(character);}} if let Some(Frame::Note {note:current,..})=self.frames.last_mut() {*current=note;}},
                Key::Backspace=>{note.pop();if let Some(Frame::Note {note:current,..})=self.frames.last_mut() {*current=note;}},
                Key::Enter if !note.trim().is_empty()=> {
                    self.frames.pop();
                    if matches!(intent,Intent::Stop(_)) {self.frames.push(Frame::Assertion {intent,note,checked:false,scroll:0});}
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
                    match parsed {Ok(actions)=>{self.frames.pop();return Effect::Submit(actions);},Err(error)=>self.fail(format!("Cannot submit: {error}"))}
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
            Page::Workers=>"Chief · Workers".into(),Page::Controls=>format!("Chief controls · {}",self.status()),
            _=>format!("Chief · {}",self.status()),
        };
        let mut lines=Vec::new();
        if let Some(error)=&self.read_error {lines.push(format!("Stale · {error}"));}
        if !self.message.is_empty() {lines.push(self.message.clone());}
        if !self.snapshot["coordination_error"].is_null() {lines.push(format!("Host failure: {}",shown(&self.snapshot["coordination_error"])));}
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
                lines.push(match page {Page::Controls=>"Chief controls".into(),Page::Workers=>"Workers · read-only; availability is reported by the host".into(),_=>String::new()});
                let rows=self.rows(page);if rows.is_empty() {lines.push("Nothing available. Missing records do not imply readiness or approval.".into());}
                let selected_index=selected.as_ref().and_then(|key|rows.iter().position(|row|row.key==*key));
                let first=selected_index.unwrap_or(0).saturating_sub(4);
                for row in rows.iter().skip(first) {lines.push(format!("{} {}",if selected.as_ref()==Some(&row.key) {">"} else {" "},row.label));}
            },
            Some(Frame::Detail {record,..})=> {
                if record["kind"]=="worker" {
                    lines.push("Availability is daemon-derived; runtime reports are not independently proven. Unknown/stale means unavailable. Claude/Codex monitor-only. Host-native live verification is still required.".into());
                    for related in self.records("team_member").into_iter().filter(|related|related["session"]["session_id"]==record["session"]["session_id"]) {lines.push(shown(&related));}
                    let age=record["observed_at_ms"].as_i64().map(|observed|((crate::util::now()*1000.0) as i64).saturating_sub(observed));
                    lines.push(format!("Observation age (ms): {}",shown(&json!(age))));
                    let admissions:Vec<_>=self.records("admission").into_iter().filter(|admission|admission["session"]==record["session"]).collect();
                    lines.push(format!("Host-native admission verification records (empty means not verified): {}",shown(&json!(admissions))));
                }
                if record["kind"]=="decision" {
                    lines.push("Source: protected host decision; chief question/recommendation are untrusted narrative, not evidence or permission.".into());
                }
                lines.push(shown(record));
            },
            Some(Frame::Note {note,..})=>lines.push(format!("Nonempty note (not consent): {}",escape_hiding(note,"",""))),
            Some(Frame::Assertion {intent,note,checked,..})=> {
                for anchor in Self::anchors(intent) {lines.push(shown(&anchor));}
                lines.push(format!("Note: {}",escape_hiding(note,"","")));
                lines.push(format!("[{}] {}",if *checked {"x"} else {" "},if matches!(intent,Intent::Stop(target) if target["kind"]=="legacy_launch") {"I confirm this exact launch can no longer execute; its worker identity may be unknown."} else {"I confirm this exact delivery can no longer reach its recipient."}));
            },
            Some(Frame::Review {content,confirm,invalid,..})=> {
                lines.push(if *invalid {"Review invalid: records changed or unavailable. Fresh review required."} else if *confirm {"CONFIRM these exact reviewed actions? y/n"} else {"Frozen review — Enter opens separate confirmation; y does nothing here."}.into());lines.push(content.clone());
            },
            Some(Frame::Summary {..})=> {
                lines.push("Requested summary only. Chief narrative and worker claims are unverified.".into());
                let summary=self.records("summary");if summary.is_empty() {lines.push("Summary unavailable — host has no result.".into());} else {for record in summary {lines.push(shown(&record));}}
            },Some(Frame::Results {content,..})=>lines.push(content.clone()),None=>{},
        }
        lines
    }
    pub fn footer(&self) -> &str {match self.frames.last() {
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
