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
/// Plain words for a known host startup reason; the host store and snapshot keep the raw text.
fn plain_startup_reason(reason: &str) -> &str {
    match reason {
        "host live OpenCode draft/queue verification is required before chief activation"=>"Waiting to confirm Chief can receive messages from your agents.",
        _=>reason,
    }
}
/// A decision the Chief raised for a stopped agent: Theo can answer it from here.
fn is_escalation(record: &Value) -> bool {record["kind"]=="decision" && record["stop_id"].is_string()}
fn suggestion(decision: &Value) -> Option<&str> {decision["suggested_answer"].as_str().filter(|suggested|!suggested.trim().is_empty())}
/// The message exactly as the stopped agent receives it, boundary text included.
fn delivered_text(answer: &str) -> String {
    crate::teams::envelope_text(&json!({"message_id":"review","kind":"chief_answer","source":"theo","text":answer}))
}
fn answer_action(decision: &Value, answer: &str) -> Value {
    ChiefView::action("answer_decision",json!({"decision":decision["id"],"expected_revision":decision["revision"],"answer":answer}))
}
/// A host-supplied string cut to one escaped line: agent-chosen text can hold newlines, terminal codes and any length.
pub(crate) fn one_line(value: &str, limit: usize) -> String {
    let escaped = escape_hiding(&value.split_whitespace().collect::<Vec<_>>().join(" "), "", "");
    if escaped.chars().count() <= limit {return escaped;}
    let mut kept: String = escaped.chars().take(limit.saturating_sub(1)).collect();
    kept.push('…');
    kept
}
/// `ses_…` and the last 6 characters of a session id, for a session without a name.
pub fn fallback_session_name(session_id: &str) -> String {
    let tail: String = session_id.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
    format!("ses_…{}",one_line(&tail,6))
}
/// `<session name> · <question> → <answer>` on one escaped line of at most `width` characters, the question and the name cut first so the answer stays readable.
pub fn answer_summary_line(session_name: &str, question: &str, answer: &str, width: usize) -> String {
    one_line(&format!("{} · {} → {}",one_line(session_name,NAME_LIMIT.min(30)),one_line(question,40),one_line(answer,LINE_LIMIT)),width)
}
/// `<habit> · <project name | everywhere>` on one escaped line of at most `width` characters, the habit cut so its scope stays readable.
pub fn habit_summary_line(text: &str, project: Option<&str>, width: usize) -> String {
    let scope=one_line(&scope_name(&project.map_or(Value::Null,|project|Value::String(project.into()))),30);
    one_line(&format!("{} · {scope}",one_line(text,width.saturating_sub(scope.chars().count()+3))),width)
}
/// When a "since" time started: `HH:MM` on `today`, `yesterday HH:MM` the day before, otherwise `Wed 14 Oct 09:05`.
pub fn since_label(since_ms: i64, today: chrono::NaiveDate) -> String {
    let Some(moment)=chrono::DateTime::from_timestamp_millis(since_ms).map(|moment|moment.with_timezone(&chrono::Local)) else {return "an unknown time".into()};
    let day=moment.date_naive();
    if day==today {moment.format("%H:%M").to_string()}
    else if today.pred_opt()==Some(day) {moment.format("yesterday %H:%M").to_string()}
    else {moment.format("%a %d %b %H:%M").to_string()}
}
/// How far back a digest or summary looks when it has no earlier one to start from.
pub const DEFAULT_LOOKBACK_MS: i64 = 24*3_600_000;
/// Answer lines the digest and the summary list before `+K more`, and the width of each.
pub const DIGEST_LINES: usize = crate::chief::store::DIGEST_ANSWER_LINES;
pub const DIGEST_LINE_WIDTH: usize = 100;
fn field(record: &Value, name: &str) -> String {one_line(record[name].as_str().unwrap_or("?"),LINE_LIMIT)}
const LINE_LIMIT: usize = 300;
const NAME_LIMIT: usize = 80;
/// How many answers the host snapshot holds at most.
const ANSWER_CAP: usize = 200;
fn delivery_word(delivery: &Value) -> String {
    match delivery.as_str() {
        Some("queued") => "waiting".into(), Some("received") => "sent".into(), Some("uncertain") => "not sure".into(),
        Some("cancelled") => "cancelled".into(), Some(other) => one_line(other,40), None => "?".into(),
    }
}
fn clock(milliseconds: &Value) -> String {
    milliseconds.as_i64().and_then(chrono::DateTime::from_timestamp_millis)
        .map(|moment|moment.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_else(||"--:--".into())
}
fn answer_title(answer: &Value, session: Option<&str>) -> String {
    match session {
        Some(name) => format!("{} · {} · {}",clock(&answer["created_at_ms"]),name,field(answer,"question")),
        None => format!("{} · {}",clock(&answer["created_at_ms"]),field(answer,"question")),
    }
}
/// Lines 1 and 2 of an answer row (`HH:MM · question`, `Chief: answer`) for the details of a session card:
/// escaped, one line each, cut to `room` cells with `…`.
pub fn recent_answer_lines(answer: &Value, room: usize) -> [String; 2] {
    [crate::app::fit(&answer_title(answer,None),room),crate::app::fit(&format!("Chief: {}",field(answer,"answer")),room)]
}
/// Why an answer cannot be overruled, in the words Theo reads; the host enforces the same rules.
fn overrule_block(answer: &Value) -> Option<&'static str> {
    if answer["source"]!="chief" {return Some("This answer is yours");}
    if !answer["overrule"].is_null() {return Some("Already overruled");}
    match answer["delivery"].as_str() {
        Some("received"|"uncertain") => None,
        Some("queued") => Some("Not sent yet"),
        Some("cancelled") => Some("Cancelled"),
        _ => Some("Delivery unknown"),
    }
}
fn correction_action(answer: &Value, correction: &str) -> Value {
    ChiefView::action("overrule",json!({"answer":answer["id"],"correction":correction}))
}
/// The message exactly as the agent receives the correction, label included.
fn correction_text(answer: &Value, correction: &str) -> String {
    let quoted = answer["answer"].as_str().map(crate::chief::store::answer_excerpt).unwrap_or_default();
    crate::teams::envelope_text(&json!({"message_id":"review","kind":"chief_correction","source":"theo","answer_excerpt":quoted,"text":correction}))
}
fn correction_explanation(answer: &Value, correction: &str) -> String {
    format!("Sends this correction to the agent that got the Chief's answer. The host adds the label below. The agent receives exactly:\n\n{}\n\nAdds a habit: {} · {}",
        correction_text(answer,correction),escape_hiding(&crate::chief::store::habit_text(correction),"",""),scope_name(&answer["project"]))
}
/// A project key as Theo reads it (its last path part), or `everywhere` for none.
fn scope_name(project: &Value) -> String {
    match project.as_str() {Some(project)=>crate::teams::view::short_project(project),None=>"everywhere".into()}
}
fn habit_source(habit: &Value) -> String {
    match habit["source_kind"].as_str() {
        Some("overrule")=>"from your overrule".into(),
        Some("theo_answer")=>"from your answer".into(),
        Some("theo_message")=>"from your message".into(),
        _=>format!("from {}",field(habit,"source_kind")),
    }
}
fn answer_explanation(answer: &str) -> String {
    format!("Sends this answer to the stopped agent and resolves the decision. The host adds the label below. The agent receives exactly:\n\n{}",delivered_text(answer))
}
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
    Answers(Option<String>), AnswerActions(Value),
    /// Habits created after this time (epoch ms, when Theo last saw them before opening the page) are new.
    Habits(i64),
}
#[derive(Clone)]
enum Intent { Resolve(Value), Answer(Value), Handled(Value), Stop(Value), Overrule(Value) }
#[derive(Clone)]
enum Operation {
    Open(Page), Inspect(Value, Option<Page>), Review(Vec<Value>, Vec<Value>, String),
    Note(Intent), Summary,
}
#[derive(Clone, Copy, PartialEq)]
pub enum Tone { Normal, Dim }
/// How a marked part of a list line is drawn: red for an alert, purple and bold for what the Chief
/// learned, or a project's colour.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mark { Alert, Attention, Learned, Project(u8) }
/// One screen line of a list, with what the terminal needs to style it. `marks` are character
/// ranges of `text` drawn in their own style.
#[derive(Clone)]
pub struct ListLine { pub text: String, pub tone: Tone, pub selected: bool, pub marks: Vec<(usize, usize, Mark)> }
impl ListLine {
    fn plain(text: String) -> Self { Self {text,tone:Tone::Normal,selected:false,marks:Vec::new()} }
}
struct Row { key: String, label: String, label_marks: Vec<(usize, usize, Mark)>, operation: Operation, more: Vec<ListLine> }
impl Row {
    fn new(key: &str, label: &str, operation: Operation) -> Self { Self {key:key.into(),label:label.into(),label_marks:Vec::new(),operation,more:Vec::new()} }
    fn height(&self) -> usize { 1+self.more.len() }
    fn record(record: &Value, operation: Operation) -> Self {
        let label = match record["kind"].as_str() {
            Some("worker") => format!("{} · {} · {} · {}",text(&record["session"]["agent"]),text(&record["session"]["project"]),
                text(&record["role"]),if record["session"]["agent"] != "opencode" {"monitor only"} else if host_flag(&record["ready"]) {"host reports available"} else {"unavailable"}),
            Some("answer") => answer_title(record,None),
            Some("decision") => format!("{} · {}",text(&record["decision_key"]),text(&record["question"])),
            _ => identity(record),
        };
        Self {key:identity(record),label,label_marks:Vec::new(),operation,more:Vec::new()}
    }
}
#[derive(Clone)]
enum Frame {
    Browse {page:Page, selected:Option<String>},
    Detail {record:Value, actions:Option<Page>, scroll:usize},
    Note {intent:Intent, note:String, scroll:usize},
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
    /// The `Chief ✓N` of the session card the answers page was opened from; 0 shows no count.
    pub card_count: usize,
    /// When Theo last saw his habits (epoch ms); `None` when never, so the last 24 hours count as new.
    pub habits_seen_ms: Option<i64>,
    /// The habit `u` restores and the row it was on, offered only until the next key.
    undo: Option<(String, String)>,
    /// The row selected before the last habit change, to return to when the host refuses it.
    selection_before_change: Option<Option<String>>,
    /// Theo's Start and the attempt id the snapshot showed when he sent it; cleared once the host shows another.
    pending_start: Option<(crate::chief::ChiefHostAction, Option<String>)>,
    startup_error: Option<String>,
    startup_scroll: usize,
    /// The delivery check Theo said did not arrive, and the one he confirmed: each answers one check once.
    check_declined: Option<String>,
    check_confirmed: Option<String>,
    check_error: Option<String>,
}
/// Where Theo's delivery check of a started Chief stands, as the host snapshot and his answer show it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeliveryCheck { Offer, Sending, Ask, Declined }
/// `Change` is sent at once, without a review: a habit edit Theo can undo. `HabitsSeen` saves when Theo saw his habits (epoch ms).
/// `SendCheck` sends Theo's test message; `ConfirmCheck` records, for that exact session, that he saw it arrive.
pub enum Effect { None, Close, Start(crate::chief::ChiefHostAction), SendCheck(crate::chief::ChiefHostAction), ConfirmCheck(crate::teams::VerifiedSession, String), Submit(Vec<crate::chief::ChiefHostAction>), Summary, Change(crate::chief::ChiefHostAction), HabitsSeen(i64) }

impl ChiefView {
    pub fn new(snapshot: Value) -> Self {
        Self {snapshot,frames:vec![Frame::Browse {page:Page::Root,selected:Some("decisions".into())}],
            message:String::new(),suspended:false,page_step:1,read_error:None,card_count:0,habits_seen_ms:None,undo:None,selection_before_change:None,
            pending_start:None,startup_error:None,startup_scroll:0,check_declined:None,check_confirmed:None,check_error:None}
    }
    fn records(&self, kind: &str) -> Vec<Value> {
        self.snapshot["records"].as_array().into_iter().flatten().filter(|record|record["kind"]==kind).cloned().collect()
    }
    /// The answers page replaces whatever is open, so the view closes back to where Theo came from.
    pub fn open_answers(&mut self, session_id: Option<String>) {
        let page=Page::Answers(session_id);
        let selected=self.rows(&page).first().map(|row|row.key.clone());
        self.frames=vec![Frame::Browse {page,selected}];
    }
    pub fn attention(snapshot: &Value) -> usize {
        snapshot["records"].as_array().into_iter().flatten().filter(|record|record["kind"]=="decision" && record["state"]=="open").count()
    }
    /// `<label> · <project>` from the host, escaped and cut to one line; the end of the id when the host has no name.
    fn session_name(&self, session_id: Option<&str>) -> String {
        let Some(session_id)=session_id else {return "unknown session".into()};
        match self.snapshot["session_names"][session_id].as_str().map(|name|one_line(name,NAME_LIMIT)).filter(|name|!name.is_empty()) {
            Some(name)=>name,
            None=>fallback_session_name(session_id),
        }
    }
    /// The host sends the newest 200 answers only: at that count there may be older ones.
    fn answers_label(&self) -> String {
        let count=self.records("answer").len();
        format!("Answers · {count}{}",if count>=ANSWER_CAP {"+"} else {""})
    }
    fn decision_row(&self, record: &Value, operation: Operation) -> Row {
        let mut row=Row::record(record,operation);
        if is_escalation(record) {
            row.label=format!("{} · {} · {}",self.session_name(record["session_id"].as_str()),field(record,"category"),field(record,"question"));
        }
        row
    }
    fn answer_row(&self, answer: &Value, name_session: bool) -> Row {
        let actions=overrule_block(answer).is_none().then(||Page::AnswerActions(answer.clone()));
        let mut row=Row::record(answer,Operation::Inspect(answer.clone(),actions));
        if name_session {row.label=answer_title(answer,Some(&self.session_name(answer["session_id"].as_str())));}
        let author=match answer["source"].as_str() {Some("chief")=>"Chief".to_string(),Some("theo")=>"Theo".to_string(),_=>field(answer,"source")};
        let mut status=delivery_word(&answer["delivery"]);
        let mut marks=Vec::new();
        if answer["delivery"]=="uncertain" {marks.push((0,status.chars().count(),Mark::Alert));}
        if !answer["overrule"].is_null() {
            status.push_str(" · overruled · ");
            let correction=delivery_word(&answer["overrule"]["delivery"]);
            if answer["overrule"]["delivery"]=="uncertain" {let start=status.chars().count();marks.push((start,start+correction.chars().count(),Mark::Alert));}
            status.push_str(&correction);
        }
        let habits=answer["habits"].as_array().cloned().unwrap_or_default();
        let details=match habits.split_first() {
            None=>format!("{status} · {}",field(answer,"reason")),
            Some((first,rest))=> {
                // A long reason would push the habits off the line.
                let mut details=format!("{status} · {} · habits: {}",one_line(answer["reason"].as_str().unwrap_or("?"),80),one_line(first["text"].as_str().unwrap_or("?"),60));
                if !rest.is_empty() {details.push_str(&format!(" +{}",rest.len()));}
                details
            },
        };
        row.more=vec![ListLine::plain(format!("{author}: {}",field(answer,"answer"))),
            ListLine {text:details,tone:Tone::Dim,selected:false,marks}];
        row
    }
    fn habits_since(&self) -> i64 {
        self.habits_seen_ms.unwrap_or_else(||chrono::Utc::now().timestamp_millis()-DEFAULT_LOOKBACK_MS)
    }
    /// Active habits, newest first: the host also sends deleted ones, for undo.
    fn active_habits(&self) -> Vec<Value> {
        let mut habits: Vec<Value>=self.records("habit").into_iter().filter(|habit|habit["state"]=="active").collect();
        habits.sort_by_key(|habit|std::cmp::Reverse(habit["created_at_ms"].as_i64().unwrap_or(0)));
        habits
    }
    fn habits_label(&self) -> String {
        let habits=self.active_habits();
        let since=self.habits_since();
        let fresh=habits.iter().filter(|habit|habit["created_at_ms"].as_i64().is_some_and(|created|created>since)).count();
        if fresh==0 {format!("Habits · {}",habits.len())} else {format!("Habits · {} · {fresh} new",habits.len())}
    }
    fn habit_row(habit: &Value, since: i64) -> Row {
        let mut row=Row::record(habit,Operation::Inspect(habit.clone(),None));
        let habit_text=one_line(habit["text"].as_str().unwrap_or("?"),crate::chief::store::HABIT_TEXT_LIMIT);
        if habit["created_at_ms"].as_i64().is_some_and(|created|created>since) {
            row.label=format!("new {habit_text}");row.label_marks=vec![(0,3,Mark::Learned)];
        } else {row.label=habit_text;}
        let scope=scope_name(&habit["project"]);
        let marks=if habit["project"].is_string() {vec![(0,scope.chars().count(),Mark::Project(crate::app::project_color(&scope)))]} else {Vec::new()};
        row.more=vec![ListLine {text:format!("{scope} · {} · {}",habit_source(habit),clock(&habit["created_at_ms"])),tone:Tone::Dim,selected:false,marks}];
        row
    }
    fn chiefs(&self) -> Vec<Value> { self.records("chief").into_iter().filter(|chief|chief["state"]!="disabled").collect() }
    fn startup_root(&self) -> bool {
        matches!(self.frames.last(),Some(Frame::Browse {page:Page::Root,..})) && self.chiefs().is_empty() && self.snapshot.get("startup").is_some()
    }
    pub fn startup_screen(&self) -> bool {
        // Existing recovery decisions must remain reachable even while Chief is off.
        self.startup_root() && !self.records("decision").iter().any(|record|record["state"]=="open")
    }
    pub fn startup_phase(&self) -> &str {
        if self.read_error.is_some() {return "unavailable";}
        if self.pending_start.is_some() {return if self.startup_error.is_some() {"unconfirmed"} else {"starting"};}
        match self.snapshot["startup"]["state"].as_str() {
            None if self.snapshot["startup"].is_null()=>"off",
            Some("ended")=>"off",
            Some("starting")=>"starting",
            Some("failed") if self.snapshot["startup"]["can_retry"]==true=>"failed",
            _=>"checking",
        }
    }
    pub fn startup_action_label(&self) -> Option<&'static str> {
        match self.startup_phase() {"off"=>Some("Start Chief"),"failed"=>Some("Try again"),"unconfirmed"=>Some("Send again"),_=>None}
    }
    pub fn startup_failed(&mut self, error: String) {self.startup_error=Some(escape_hiding(&error,"",""));}
    pub fn startup_acknowledged(&mut self) {self.startup_error=None;}
    pub fn suspend_pending_start(&mut self) -> bool {
        if self.pending_start.is_none() {return false;}
        self.frames=vec![Frame::Browse {page:Page::Root,selected:Some("decisions".into())}];
        self.suspend();
        true
    }
    /// Offered only on Checking, and only for the exact live session the host names as waiting for its check.
    fn delivery_check(&self) -> Option<DeliveryCheck> {
        let startup=&self.snapshot["startup"];
        if !self.startup_screen() || self.startup_phase()!="checking" || !startup["check_session"].is_object() {return None;}
        let check=&startup["check"];
        let id=check["id"].as_str();
        Some(match check["state"].as_str() {
            None=>DeliveryCheck::Offer,
            Some("queued"|"submitting")=>DeliveryCheck::Sending,
            Some("received") if check["session"]==startup["check_session"] && id!=self.check_declined.as_deref() && id!=self.check_confirmed.as_deref()=>DeliveryCheck::Ask,
            Some("received") if id==self.check_confirmed.as_deref()=>DeliveryCheck::Sending,
            _=>DeliveryCheck::Declined,
        })
    }
    fn delivery_check_key(&mut self, key: &Key) -> Effect {
        let Some(stage)=self.delivery_check() else {return Effect::None};
        let startup=&self.snapshot["startup"];
        let check_id=startup["check"]["id"].as_str().map(str::to_string);
        match (key,stage) {
            (Key::Char('t'),DeliveryCheck::Offer|DeliveryCheck::Declined)=>{
                let attempt=startup["id"].as_str().unwrap_or_default().to_string();
                // Named after the check it follows: a repeated key replays the same request on the host.
                let request_id=format!("chief-check-{attempt}-after-{}",check_id.as_deref().unwrap_or("none"));
                self.check_error=None;
                Effect::SendCheck(crate::chief::ChiefHostAction::SendCheck {request_id,attempt})
            },
            (Key::Char('y'),DeliveryCheck::Ask)=>{
                let Some(id)=check_id else {return Effect::None};
                let Ok(session)=serde_json::from_value::<crate::teams::VerifiedSession>(startup["check"]["session"].clone()) else {return Effect::None};
                self.check_confirmed=Some(id.clone());self.check_error=None;
                Effect::ConfirmCheck(session,format!("Theo confirmed in the Cockpit that delivery check {id} (native msg_cockpit{id}) appeared in the Chief pane."))
            },
            (Key::Char('n'),DeliveryCheck::Ask)=>{self.check_declined=check_id;self.check_error=None;Effect::None},
            _=>Effect::None,
        }
    }
    pub fn check_failed(&mut self, error: String) {
        self.check_confirmed=None;
        self.check_error=Some(escape_hiding(&error,"",""));
    }
    pub fn startup_lines(&self) -> Vec<String> {
        let (heading, explanation, helper)=match self.startup_phase() {
            "off"=>("Chief is off","Chief handles routine questions from your agents and brings important decisions to you.","Creates its own OpenCode session in a dedicated folder."),
            "starting"=>("Starting Chief…","Opening its OpenCode session.","You can leave this view. We'll keep track of this launch."),
            "failed"=>("Chief could not start","","No session remains. You can try again."),
            "unavailable"=>("Chief status unavailable","Waiting for a trustworthy host snapshot.","You can leave this view while we check."),
            "unconfirmed"=>("Checking Chief…","The host has not confirmed this start yet.","Sends the same request again; it cannot start a second Chief."),
            _=>("Checking Chief…","Its session may have started. Waiting to confirm that Chief is ready.","You can leave this view while we check."),
        };
        let mut lines=vec![heading.into(),String::new(),explanation.into()];
        if let Some(error)=self.read_error.as_deref().or(self.startup_error.as_deref()).or(self.snapshot["startup"]["error"].as_str().map(plain_startup_reason)) {
            lines.push(escape_hiding(error,"",""));
        }
        lines.push(String::new());
        match self.delivery_check() {
            Some(DeliveryCheck::Offer)=>lines.extend(["[t] Send test message".into(),String::new()]),
            Some(DeliveryCheck::Declined)=>lines.extend(["Not verified. You can send another test message.".into(),String::new(),"[t] Send test message".into(),String::new()]),
            Some(DeliveryCheck::Sending)=>lines.extend(["Sending test message…".into(),String::new()]),
            Some(DeliveryCheck::Ask)=>lines.extend(["Test message sent. Look at the Chief pane: did it appear?".into(),String::new(),
                "[y] It arrived".into(),"[n] Didn't arrive".into(),String::new()]),
            None=>{},
        }
        if let Some(error)=&self.check_error {lines.extend([error.clone(),String::new()]);}
        if let Some(action)=self.startup_action_label() {lines.push(format!("[c] {action}"));lines.push(String::new());}
        else if self.startup_phase()=="starting" {lines.push("Starting…".into());lines.push(String::new());}
        lines.push(helper.into());
        lines
    }
    pub fn status(&self) -> String {
        if self.read_error.is_some() {return "Unknown / host review unavailable — retained records are stale".into();}
        let chiefs=self.chiefs();
        match chiefs.as_slice() {
            [] if self.snapshot.get("startup").is_some()=>match self.startup_phase() {"off"=>"Off","starting"=>"Starting","failed"=>"Could not start",_=>"Checking"}.into(),
            [] => "Off".into(),
            [chief] if !host_flag(&chief["live_verified"]) => format!("Unavailable · retained {} grant",text(&chief["state"])),
            [chief] if self.unavailable_reason(chief).is_some() => format!("Unavailable · retained {} grant · {}",text(&chief["state"]),self.unavailable_reason(chief).expect("reason checked")),
            [chief] if chief["state"]=="paused" => "Paused · workers continue".into(),
            [chief] if chief["state"]=="active" => "Active".into(),
            _ => "Unknown chief state — host review required".into(),
        }
    }
    pub fn status_lines(&self) -> Vec<String> {
        let status = self.status();
        let summary = match (self.snapshot["today"]["answered"].as_u64(), self.snapshot["today"]["escalated"].as_u64()) {
            (Some(answered), Some(escalated)) if self.read_error.is_none() => format!("{status} · answered {answered} today · {escalated} sent to you"),
            _ => format!("{status} · today's counts unavailable"),
        };
        let mut lines = vec![summary];
        if let Some(chief) = self.chiefs().first() {
            lines.push(format!("Runs as {}", self.session_name(chief["session"]["session_id"].as_str())));
        }
        if let Some(answer) = self.records("answer").iter().filter(|answer| answer["source"] == "chief" && answer["delivery"] != "cancelled")
            .max_by_key(|answer| answer["created_at_ms"].as_i64()) {
            lines.push(format!("Last: {} · {} · {}", field(answer, "answer"), self.session_name(answer["session_id"].as_str()), clock(&answer["created_at_ms"])));
        }
        lines
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
        if let Some((_,seen))=&self.pending_start {
            if !self.chiefs().is_empty() || self.snapshot["startup"]["id"].as_str().is_some_and(|id|Some(id)!=seen.as_deref()) {
                self.pending_start=None;self.startup_error=None;self.startup_scroll=0;
            }
        }
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
                        Some("decision")=>Some(Page::DecisionActions(current.clone())),
                        Some("answer")=>overrule_block(current).is_none().then(||Page::AnswerActions(current.clone())),
                        _=>actions.clone(),
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
        self.undo=None;
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
        (*key==Key::Char('c') && self.startup_root() && self.startup_action_label().is_some())
            || (*key==Key::Char('t') && matches!(self.delivery_check(),Some(DeliveryCheck::Offer|DeliveryCheck::Declined)))
            || (*key==Key::Char('y') && self.delivery_check()==Some(DeliveryCheck::Ask))
            || (*key==Key::Char('y') && matches!(self.frames.last(),Some(Frame::Review {confirm:true,invalid:false,..})))
            || (*key==Key::Ctrl('s') && matches!(self.frames.last(),Some(Frame::Note {intent:Intent::Overrule(_),note,..}) if !note.trim().is_empty()))
    }
    fn open(&mut self, page: Page) {
        let selected=self.rows(&page).first().map(|row|row.key.clone()); self.frames.push(Frame::Browse {page,selected});
    }
    fn standalone(&self, worker: &Value) -> bool {
        !self.records("team_member").iter().any(|member|member["session"]["session_id"]==worker["session"]["session_id"])
            && !self.chiefs().iter().any(|chief|chief["session"]["session_id"]==worker["session"]["session_id"])
    }
    fn picker_row(&self, worker: Value) -> Row {
        let reason = if !self.chiefs().is_empty() {Some("disable the current Chief in Chief controls first")}
            else if worker["session"]["agent"] != "opencode" {Some("no native inbox")}
            else if !host_flag(&worker["ready"]) {Some("waiting for verified availability")}
            else if worker["blocked_by"].as_array().is_none_or(|blockers| !blockers.is_empty()) {Some("host reports this session blocked")}
            else {None};
        let operation = match reason {
            Some(_) => Operation::Inspect(worker.clone(), None),
            None => Operation::Review(vec![Self::action("enable",json!({"session":worker["session"]}))],vec![worker.clone()],
                "Cross-project operational status and bounded briefs only. No extra file access, credentials, transcripts, raw approval payloads or permissions. Never starts an agent.".into()),
        };
        let mut row = Row::record(&worker, operation);
        row.label = self.session_name(worker["session"]["session_id"].as_str());
        row.more = vec![ListLine::plain(format!("{} · {}",field(&worker["session"],"agent"),
            reason.map(|reason|format!("can't be Chief: {reason}")).unwrap_or_else(||"ready".into())))];
        row
    }
    fn action(action: &str, fields: Value) -> Value {
        let mut payload=fields; payload["action"]=json!(action); payload["request_id"]=json!(format!("chief-ui-{}",crate::util::token_hex(12))); payload
    }
    fn mutation(label: &str, action: Value, anchors: Vec<Value>, explanation: &str) -> Row {
        Row::new(label,label,Operation::Review(vec![action],anchors,explanation.into()))
    }
    fn recovery_target(&self, decision: &Value) -> Option<Value> {
        self.records("inbox").into_iter().find(|delivery|decision["scope_key"]==format!("host-delivery-{}",text(&delivery["id"])) && matches!(delivery["state"].as_str(),Some("uncertain"|"failed")))
            .or_else(||self.records("legacy_launch").into_iter().find(|launch|decision["scope_key"]==format!("host-launch-{}",text(&launch["id"])) && launch["state"]!="released"))
    }
    fn rows(&self, page: &Page) -> Vec<Row> {
        use Operation::*;
        match page {
            Page::Root => {
                let mut rows=vec![Row::new("decisions",&format!("Needs you · {}",Self::attention(&self.snapshot)),Open(Page::Decisions)),
                Row::new("answers",&self.answers_label(),Open(Page::Answers(None))),
                Row::new("habits",&self.habits_label(),Open(Page::Habits(self.habits_since()))),
                Row::new("controls","Chief controls",Open(Page::Controls)),
                 Row::new("workers","Workers",Open(Page::Workers)),Row::new("summary","Request summary",Summary)];
                if self.chiefs().is_empty() && self.snapshot.get("startup").is_some() {rows.retain(|row|row.key!="controls");}
                for decision in self.records("decision").into_iter().filter(|decision|decision["state"]=="open" && self.recovery_target(decision).is_some()) {
                    let mut row=self.decision_row(&decision,Inspect(decision.clone(),Some(Page::DecisionActions(decision.clone()))));
                    row.label=format!("! {}",field(&decision,"question"));
                    row.label_marks=vec![(0,row.label.chars().count(),Mark::Attention)];
                    row.more=vec![ListLine::plain("y it arrived · x stop safely · i details".into())];
                    rows.push(row);
                }
                rows
            },
            Page::Workers => self.records("worker").into_iter().map(|record|Row::record(&record,Inspect(record.clone(),None))).collect(),
            Page::Decisions => self.records("decision").into_iter().filter(|record|record["state"]=="open")
                .map(|record|self.decision_row(&record,Inspect(record.clone(),Some(Page::DecisionActions(record.clone()))))).collect(),
            Page::Answers(session) => {
                let mut answers: Vec<Value> = self.records("answer").into_iter()
                    .filter(|answer|session.as_deref().is_none_or(|id|answer["session_id"].as_str()==Some(id))).collect();
                answers.sort_by_key(|answer|std::cmp::Reverse(answer["created_at_ms"].as_i64().unwrap_or(0)));
                answers.iter().map(|answer|self.answer_row(answer,session.is_none())).collect()
            },
            Page::Habits(since) => self.active_habits().iter().map(|habit|Self::habit_row(habit,*since)).collect(),
            Page::AnswerActions(answer) => vec![Row::new("overrule","Overrule",Note(Intent::Overrule(answer.clone())))],
            Page::Controls => {
                let chiefs=self.chiefs();
                if chiefs.is_empty() {return if self.snapshot.get("startup").is_some() {Vec::new()} else {vec![Row::new("select","Select chief",Open(Page::Sessions))]};}
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
                .map(|worker|self.picker_row(worker)).collect(),
            Page::DecisionActions(decision) => {
                let mut rows=Vec::new();
                if is_escalation(decision) {
                    if let Some(suggested)=suggestion(decision) {
                        rows.push(Self::mutation("Send the suggestion",answer_action(decision,suggested),vec![decision.clone()],&answer_explanation(suggested)));
                    }
                    rows.push(Row::new("answer","Write my own answer",Note(Intent::Answer(decision.clone()))));
                }
                rows.push(Row::new("resolve","Resolve with note",Note(Intent::Resolve(decision.clone()))));
                if let Some(target)=self.recovery_target(decision) {
                    rows.push(Row::new("recovery","Recovery",Open(Page::Recovery(target))));
                }
                rows
            },
            Page::Recovery(target) => vec![Row::new("handled","Confirm handled",Note(Intent::Handled(target.clone()))),
                Row::new("stop","Confirm safe stop and cancel",Note(Intent::Stop(target.clone())))],
        }
    }
    fn anchors(intent: &Intent) -> Vec<Value> {match intent {
        Intent::Resolve(record)|Intent::Answer(record)|Intent::Handled(record)|Intent::Stop(record)|Intent::Overrule(record)=>vec![record.clone()],
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
            Intent::Answer(decision)=>answer_action(&decision,&note),
            Intent::Overrule(answer)=>correction_action(&answer,&note),
            Intent::Handled(target)=>Self::recovery_action(&target,json!({"kind":"handled","note":note})),
            Intent::Stop(target)=>Self::recovery_action(&target,json!({"kind":"safe_stop","note":note,"confirmed_stopped":true})),
        };
        let explanation=match &intent {Intent::Resolve(_)=>"Record a resolution note only. Grants no authority and does not resolve a permission prompt.".to_string(),
            Intent::Answer(_)=>answer_explanation(&note),
            Intent::Overrule(answer)=>correction_explanation(answer,&note),
            Intent::Handled(_)=>"Record handled; retain ownership. This is not completion. Never blindly resend or reassign.".to_string(),
            Intent::Stop(_)=>"You explicitly confirm the exact launch or delivery recipient can no longer execute. Host validation remains authoritative; unknown/unresponsive ownership stays blocked.".to_string()};
        self.begin_review(vec![action],Self::anchors(&intent),explanation);
    }
    /// A note is at most 1,000 characters; an answer to a stopped agent may be as long as the Chief's.
    fn note_limit(intent: &Intent) -> usize {match intent {Intent::Answer(_)=>4000,Intent::Overrule(_)=>2000,_=>1000}}
    fn recovery_action(target: &Value, evidence: Value) -> Value {
        if target["kind"]=="inbox" {Self::action("reconcile_delivery",json!({"message":target["id"],"expected_revision":target["revision"],"evidence":evidence}))}
        else {Self::action("reconcile_launch",json!({"token":target["id"],"expected_revision":target["revision"],"evidence":evidence}))}
    }
    /// Opens the Habits page: it marks as new what Theo had not seen, and from now on nothing is.
    /// Seen means read by the host: a stale page, or one without a read time, marks nothing as seen.
    fn open_habits(&mut self) -> Effect {
        let since=self.habits_since();
        self.open(Page::Habits(since));
        let Some(read_at_ms)=self.snapshot["read_at_ms"].as_i64().filter(|_|self.read_error.is_none()) else {return Effect::None};
        self.habits_seen_ms=self.habits_seen_ms.max(Some(read_at_ms));
        Effect::HabitsSeen(read_at_ms)
    }
    fn request_id() -> String {format!("chief-ui-{}",crate::util::token_hex(12))}
    fn select(&mut self, key: Option<String>) {
        if let Some(Frame::Browse {selected,..})=self.frames.last_mut() {*selected=key;}
    }
    /// A habit change the host refused: the undo offer goes, and the selection returns to the row it was on.
    pub fn habit_change_failed(&mut self, message: String) {
        if let Some(previous)=self.selection_before_change.take() {self.select(previous);}
        self.fail(message);
    }
    fn current_selection(&self) -> Option<String> {
        match self.frames.last() {Some(Frame::Browse {selected,..})=>selected.clone(),_=>None}
    }
    fn habit_key(&mut self, key: &Key, rows: &[Row], index: Option<usize>, undo: Option<(String,String)>) -> Effect {
        use crate::chief::ChiefHostAction::*;
        self.selection_before_change=Some(self.current_selection());
        if *key==Key::Char('u') {
            let Some((habit,row))=undo else {return Effect::None};
            self.select(Some(row));
            return Effect::Change(RestoreHabit {request_id:Self::request_id(),habit});
        }
        let Some(index)=index else {return Effect::None};
        let Operation::Inspect(habit,_)=&rows[index].operation else {return Effect::None};
        let Some(id)=habit["id"].as_str().map(str::to_string) else {return Effect::None};
        match key {
            Key::Char('x')=> {
                let next=rows.get(index+1).or_else(||index.checked_sub(1).and_then(|previous|rows.get(previous))).map(|row|row.key.clone());
                self.select(next);
                self.undo=Some((id.clone(),rows[index].key.clone()));
                self.message="Deleted · u undo".into();
                Effect::Change(DeleteHabit {request_id:Self::request_id(),habit:id})
            },
            Key::Char('p')=>Effect::Change(RescopeHabit {request_id:Self::request_id(),habit:id,everywhere:habit["project"].is_string()}),
            _=>Effect::None,
        }
    }
    pub fn handle(&mut self, key: Key) -> Effect {
        if self.startup_root() {
            if self.startup_screen() && matches!(key,Key::Char('t'|'y'|'n')) {return self.delivery_check_key(&key);}
            if key==Key::Char('c') {
                // Resending the unconfirmed request keeps its id: the host replays it instead of starting twice.
                if self.startup_phase()=="unconfirmed" {
                    return Effect::Start(self.pending_start.as_ref().expect("unconfirmed start is pending").0.clone());
                }
                if self.startup_action_label().is_some() {
                    let seen=self.snapshot["startup"]["id"].as_str().map(str::to_string);
                    let previous_attempt=seen.clone().filter(|_|self.snapshot["startup"]["state"]=="failed");
                    let action=crate::chief::ChiefHostAction::Start {request_id:format!("chief-ui-{}",crate::util::token_hex(12)),previous_attempt};
                    self.pending_start=Some((action.clone(),seen));self.startup_error=None;self.startup_scroll=0;
                    return Effect::Start(action);
                }
                return Effect::None;
            }
            if key==Key::Char('p') {return Effect::None;}
            if self.startup_screen() {
                match key {
                    Key::Esc=>return Effect::Close,
                    Key::PageDown=>self.startup_scroll=self.startup_scroll.saturating_add(self.page_step),
                    Key::PageUp=>self.startup_scroll=self.startup_scroll.saturating_sub(self.page_step),
                    Key::Char('l'|'h'|'r')=>{},
                    _=>return Effect::None,
                }
                if matches!(key,Key::PageDown|Key::PageUp) {return Effect::None;}
            }
        }
        self.message.clear();
        let undo=self.undo.take();
        self.selection_before_change=None;
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
                    Key::Char('h') if matches!(page,Page::Root)=>return self.open_habits(),
                    Key::Char('y'|'x'|'i') if matches!(page,Page::Root)=> {
                        if let Some(index)=index {
                            if let Operation::Inspect(record,actions)=rows[index].operation.clone() {
                                if key==Key::Char('i') {self.frames.push(Frame::Detail {record,actions,scroll:0});}
                                else if let Some(target)=self.recovery_target(&record) {
                                    self.frames.push(Frame::Note {intent:if key==Key::Char('y') {Intent::Handled(target)} else {Intent::Stop(target)},note:String::new(),scroll:0});
                                }
                            }
                        }
                    },
                    Key::Char('l') if matches!(page,Page::Root)=>self.open(Page::Answers(None)),
                    Key::Char('p') if matches!(page,Page::Root)=>self.open(Page::Sessions),
                    Key::Char('r') if matches!(page,Page::Root)=>{self.frames.push(Frame::Summary {scroll:0});return Effect::Summary;},
                    Key::Char('c') if matches!(page,Page::Root)=> {
                        if let Some(control)=self.rows(&Page::Controls).into_iter().next() {
                            match control.operation {
                                Operation::Review(actions,anchors,explanation)=>self.begin_review(actions,anchors,explanation),
                                Operation::Open(page)=>self.open(page),
                                _=>{},
                            }
                        }
                    },
                    Key::Char('x'|'u'|'p') if matches!(page,Page::Habits(_))=>return self.habit_key(&key,&rows,index,undo),
                    Key::Enter => if let Some(index)=index {match rows[index].operation.clone() {
                        Operation::Open(Page::Habits(_))=>return self.open_habits(),
                        Operation::Open(page)=>self.open(page),
                        Operation::Inspect(record,actions)=>self.frames.push(Frame::Detail {record,actions,scroll:0}),
                        Operation::Review(actions,anchors,explanation)=>self.begin_review(actions,anchors,explanation),
                        Operation::Note(intent)=>self.frames.push(Frame::Note {intent,note:String::new(),scroll:0}),
                        Operation::Summary=>{self.frames.push(Frame::Summary {scroll:0});return Effect::Summary;},
                    }},
                    Key::Char('o') if matches!(page,Page::Answers(_))=>if let Some(index)=index {
                        if let Operation::Inspect(answer,_)=&rows[index].operation {
                            match overrule_block(answer) {
                                Some(reason)=>self.message=format!("Cannot overrule: {reason}."),
                                None=>self.frames.push(Frame::Note {intent:Intent::Overrule(answer.clone()),note:String::new(),scroll:0}),
                            }
                        }
                    },
                    Key::Esc=>{self.frames.pop();},
                    _=>{},
                }
            },
            Frame::Detail {record,actions,..} => match key {
                Key::Enter=>if let Some(page)=actions {if self.current(&record) {self.open(page);} else {self.message="Record changed or disappeared. Return and inspect again.".into();}},
                Key::Esc=>{self.frames.pop();},
                _=>self.scroll(&key),
            },
            Frame::Note {intent,mut note,..} => match key {
                Key::Ctrl('s') if matches!(intent,Intent::Overrule(_)) && !note.trim().is_empty()=> {
                    let Intent::Overrule(answer)=intent else {unreachable!()};
                    if self.read_error.is_some() || !self.current(&answer) || overrule_block(&answer).is_some() {
                        self.message="Answer changed or unavailable. Cancel and open it again.".into();
                        return Effect::None;
                    }
                    let action=crate::chief::ChiefHostAction::Overrule {request_id:Self::request_id(),answer:answer["id"].as_str().expect("answer has identity").into(),correction:note};
                    if let Err(error)=action.validate() {self.fail(error.message);return Effect::None;}
                    self.frames.pop();
                    return Effect::Submit(vec![action]);
                },
                Key::Char(character) if !character.is_control()=>{if note.chars().count()<Self::note_limit(&intent) {note.push(character);} if let Some(Frame::Note {note:current,scroll,..})=self.frames.last_mut() {*current=note;*scroll=usize::MAX;}},
                Key::Paste(paste)=>{for character in paste.chars().filter(|character|!character.is_control()) {if note.chars().count()<Self::note_limit(&intent) {note.push(character);}} if let Some(Frame::Note {note:current,scroll,..})=self.frames.last_mut() {*current=note;*scroll=usize::MAX;}},
                Key::Backspace=>{note.pop();if let Some(Frame::Note {note:current,scroll,..})=self.frames.last_mut() {*current=note;*scroll=usize::MAX;}},
                Key::Enter if !note.trim().is_empty()=> {
                    self.frames.pop();
                    if matches!(intent,Intent::Stop(_)) {self.frames.push(Frame::Assertion {intent,note,checked:false,scroll:0});}
                    else {self.finish_note(intent,note);}
                },
                Key::Esc=>{self.frames.pop();},
                _=>self.scroll(&key),
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
        let scroll=match self.frames.last_mut() {Some(Frame::Note {scroll,..}|Frame::Detail {scroll,..}|Frame::Review {scroll,..}|Frame::Assertion {scroll,..}|Frame::Summary {scroll}|Frame::Results {scroll,..})=>scroll,_=>return};
        match key {Key::PageDown=>*scroll=scroll.saturating_add(self.page_step),Key::PageUp=>*scroll=scroll.saturating_sub(self.page_step),_=>{}}
    }
    pub fn clamp_scroll(&mut self, maximum: usize) -> usize {
        if self.startup_screen() {self.startup_scroll=self.startup_scroll.min(maximum);return self.startup_scroll;}
        match self.frames.last_mut() {Some(Frame::Note {scroll,..}|Frame::Detail {scroll,..}|Frame::Review {scroll,..}|Frame::Assertion {scroll,..}|Frame::Summary {scroll}|Frame::Results {scroll,..})=>{*scroll=(*scroll).min(maximum);*scroll},_=>0}
    }
    pub fn is_list(&self) -> bool {!self.startup_screen() && matches!(self.frames.last(),Some(Frame::Browse {..}))}
    pub fn list_window(&self, height: usize) -> (String,Vec<ListLine>) {
        let Some(Frame::Browse {page,selected})=self.frames.last() else {return (String::new(),Vec::new())};
        let caption=match page {
            Page::Sessions=>"Pick another Chief".into(),
            Page::Workers=>"Chief · Workers".into(),Page::Controls=>format!("Chief controls · {}",self.status()),
            Page::Answers(Some(session))=>match self.card_count {
                0=>format!("Answers · {}",self.session_name(Some(session))),
                count=>format!("Answers · {} · Chief ✓{count} this task",self.session_name(Some(session))),
            },
            Page::Answers(None)=>"Chief · Answers".into(),
            Page::Habits(_)=>"Chief · Habits".into(),
            _=>format!("Chief · {}",self.status()),
        };
        let mut lines=Vec::new();
        if let Some(error)=&self.read_error {lines.push(ListLine::plain(format!("Stale · {error}")));}
        if !self.message.is_empty() {lines.push(ListLine::plain(self.message.clone()));}
        if !self.snapshot["coordination_error"].is_null() {lines.push(ListLine::plain(format!("Host failure: {}",shown(&self.snapshot["coordination_error"]))));}
        if matches!(page,Page::Root) && height>12 {
            lines.extend(self.status_lines().into_iter().map(ListLine::plain));
            lines.push(ListLine::plain(if self.startup_root() {self.footer()} else {"c pause/resume · p pick Chief · r summary now"}.into()));
            lines.push(ListLine::plain("l answer log · h habits".into()));
        }
        let available=height.saturating_sub(lines.len()).max(1);
        let rows=self.rows(page);
        if rows.is_empty() {lines.push(ListLine::plain(match page {
            Page::Answers(_)=>"No Chief answers yet.".into(),
            Page::Habits(_)=>"No habits yet. The Chief learns them from your answers and overrules.".into(),
            _=>"Nothing available. Missing records never imply readiness.".into(),
        }));}
        let position=selected.as_ref().and_then(|key|rows.iter().position(|row|row.key==*key)).unwrap_or(0);
        let mut first=position;
        let mut used=rows.get(position).map_or(0,Row::height);
        while first>0 && used+rows[first-1].height()<=available {first-=1;used+=rows[first].height();}
        let limit=lines.len()+available;
        for row in rows.iter().skip(first) {
            let is_selected=selected.as_ref()==Some(&row.key);
            let shifted=|marks: &[(usize,usize,Mark)]| marks.iter().map(|(start,end,mark)|(start+2,end+2,*mark)).collect();
            lines.push(ListLine {selected:is_selected,marks:shifted(&row.label_marks),..ListLine::plain(format!("{} {}",if is_selected {">"} else {" "},row.label))});
            lines.extend(row.more.iter().map(|line|ListLine {text:format!("  {}",line.text),selected:is_selected,marks:shifted(&line.marks),..line.clone()}));
            if lines.len()>=limit {break;}
        }
        lines.truncate(limit);
        (caption,lines)
    }
    pub fn detail(&self) -> Vec<String> {
        if self.startup_screen() {return self.startup_lines();}
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
                for row in rows.iter().skip(first) {
                    lines.push(format!("{} {}",if selected.as_ref()==Some(&row.key) {">"} else {" "},row.label));
                    lines.extend(row.more.iter().map(|line|format!("  {}",line.text)));
                }
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
                if is_escalation(record) {
                    lines.push(format!("Session: {}",self.session_name(record["session_id"].as_str())));
                    lines.push(format!("Escalation · category: {}",text(&record["category"])));
                    lines.push(format!("Question the agent asked: {}",text(&record["question"])));
                    lines.push(format!("Why the Chief escalated: {}",text(&record["recommendation"])));
                    lines.push(match suggestion(record) {Some(suggested)=>format!("Suggested answer: {}",escape_hiding(suggested,"","")),None=>"No suggested answer.".into()});
                }
                if record["kind"]=="answer" {lines.extend(self.answer_detail(record));}
                else if record["kind"]=="habit" {lines.extend(self.habit_detail(record));}
                else if !is_escalation(record) {lines.push(shown(record));}
            },
            Some(Frame::Note {intent:Intent::Overrule(answer),note,..})=> {
                lines.push(format!("Overrule: {}",text(&answer["question"])));
                lines.push(format!("Chief answered: {}",text(&answer["answer"])));
                lines.push(String::new());
                lines.push(format!("Your correction: {}",escape_hiding(note,"","")));
                lines.push(format!("Becomes a habit ({}): {}",scope_name(&answer["project"]),escape_hiding(&crate::chief::store::habit_text(note),"","")));
            },
            Some(Frame::Note {intent:Intent::Answer(_),note,..})=>lines.push(format!("Your answer to the stopped agent (Enter reviews it; nothing is sent yet): {}",escape_hiding(note,"",""))),
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
                let summary=self.records("summary");
                let since_ms=summary.first().and_then(|record|record["previous_completed_at_ms"].as_i64())
                    .unwrap_or_else(||chrono::Utc::now().timestamp_millis()-DEFAULT_LOOKBACK_MS);
                if summary.is_empty() {lines.push("Summary unavailable — host has no result.".into());} else {for record in summary {lines.push(shown(&record));}}
                lines.push(String::new());
                lines.extend(self.answers_since_lines(since_ms));
            },Some(Frame::Results {content,..})=>lines.push(content.clone()),None=>{},
        }
        lines
    }
    /// The Chief's answers after `since_ms` as the summary lists them: a heading, at most 5 newest lines, then how many more.
    fn answers_since_lines(&self, since_ms: i64) -> Vec<String> {
        let all=self.records("answer");
        let mut chief_answers: Vec<&Value>=all.iter()
            .filter(|answer|answer["source"]=="chief" && answer["delivery"]!="cancelled" && answer["created_at_ms"].as_i64().is_some_and(|created|created>since_ms)).collect();
        chief_answers.sort_by_key(|answer|std::cmp::Reverse(answer["created_at_ms"].as_i64()));
        let since=since_label(since_ms,chrono::Local::now().date_naive());
        if chief_answers.is_empty() {return vec![format!("No Chief answers since {since}.")];}
        let mut lines=vec![format!("Answers since {since}")];
        for answer in chief_answers.iter().take(DIGEST_LINES) {
            lines.push(format!("  {}",answer_summary_line(&self.session_name(answer["session_id"].as_str()),answer["question"].as_str().unwrap_or("?"),
                answer["answer"].as_str().unwrap_or("?"),DIGEST_LINE_WIDTH-2)));
        }
        let hidden=chief_answers.len().saturating_sub(DIGEST_LINES);
        if hidden>0 {
            let capped=all.len()>=ANSWER_CAP && all.iter().all(|answer|answer["created_at_ms"].as_i64().is_some_and(|created|created>since_ms));
            lines.push(format!("  +{hidden} more{}",if capped {" (this snapshot holds the newest 200 answers)"} else {""}));
        }
        lines
    }
    /// Where a habit came from: the overrule and the answer it corrected, Theo's answer, or the stop that showed his message.
    fn habit_detail(&self, habit: &Value) -> Vec<String> {
        let mut lines=vec![format!("Habit: {}",text(&habit["text"])),format!("Applies to: {}",scope_name(&habit["project"]))];
        let answers=self.records("answer");
        let not_held="It is older than the newest 200 answers this view holds.";
        match habit["source_kind"].as_str() {
            Some("overrule")=> {
                lines.push("From your overrule".into());
                match answers.iter().find(|answer|answer["overrule"]["id"]==habit["source_id"]) {
                    Some(answer)=>lines.extend([format!("Your correction: {}",text(&answer["overrule"]["correction"])),
                        format!("The Chief's answer it corrected: {}",text(&answer["answer"])),format!("Question the agent asked: {}",text(&answer["question"]))]),
                    None=>lines.push(format!("The overruled answer is not shown. {not_held}")),
                }
            },
            Some("theo_answer")=> {
                lines.push("From your answer".into());
                match answers.iter().find(|answer|answer["id"]==habit["source_id"]) {
                    Some(answer)=>lines.extend([format!("Your answer: {}",text(&answer["answer"])),format!("Question the agent asked: {}",text(&answer["question"]))]),
                    None=>lines.push(format!("Your answer is not shown. {not_held}")),
                }
            },
            Some("theo_message")=> {
                lines.push("From your message in a stopped session".into());
                if let Some(stopped)=habit["stopped_at_ms"].as_i64() {lines.push(format!("Stopped at {}",since_label(stopped,chrono::Local::now().date_naive())));}
            },
            _=>lines.push(format!("Source: {}",text(&habit["source_kind"]))),
        }
        lines.push(format!("Session: {}",self.session_name(habit["session_id"].as_str())));
        if let Some(created)=habit["created_at_ms"].as_i64() {lines.push(format!("Added {}",since_label(created,chrono::Local::now().date_naive())));}
        if let Some(reason)=habit["reason"].as_str().filter(|reason|!reason.trim().is_empty()) {lines.push(format!("Why the Chief added it: {}",escape_hiding(reason,"","")));}
        lines
    }
    fn answer_detail(&self, answer: &Value) -> Vec<String> {
        let mut lines=vec![
            format!("Session: {}",self.session_name(answer["session_id"].as_str())),
            format!("{} · {}",clock(&answer["created_at_ms"]),delivery_word(&answer["delivery"])),
            format!("Question the agent asked: {}",text(&answer["question"])),
            format!("{} answered: {}",if answer["source"]=="theo" {"Theo"} else {"Chief"},text(&answer["answer"])),
            format!("Reason: {}",text(&answer["reason"])),
        ];
        if !answer["overrule"].is_null() {
            lines.push(format!("Overruled ({}): {}",delivery_word(&answer["overrule"]["delivery"]),text(&answer["overrule"]["correction"])));
        }
        lines.push(match overrule_block(answer) {Some(reason)=>format!("Overrule: not possible. {reason}."),None=>"Enter opens Overrule.".into()});
        lines
    }
    pub fn footer(&self) -> &str {
        if self.startup_screen() {return "PgUp/Dn scroll · esc back";}
        if self.startup_root() {return match self.startup_phase() {
            "off"=>"c Start Chief · l log · h habits · ⏎ open · esc back",
            "failed"=>"c Try again · l log · h habits · ⏎ open · esc back",
            "unconfirmed"=>"c Send again · l log · h habits · ⏎ open · esc back",
            _=>"Startup pending · l log · h habits · ⏎ open · esc back",
        };}
        match self.frames.last() {
        Some(Frame::Browse {page:Page::Root,..})=>"c pause/resume · p pick · r summary · l log · h habits · ⏎ open · esc back",
        Some(Frame::Note {intent:Intent::Overrule(_),..})=>"⌘S send · PgUp/Dn scroll · Enter review · Esc cancel",
        Some(Frame::Browse {page:Page::Answers(_),..})=>"o overrule · ⏎ reason · esc back",
        Some(Frame::Browse {page:Page::Habits(_),..})=>"x delete · p project/everywhere · ⏎ source · esc back",
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
