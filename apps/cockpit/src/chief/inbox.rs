//! The Chief inbox (DESIGN.md "Chief inbox"): one escalated question at a time, its suggestion done
//! step by step, and only the last step sends the exact message shown.
use crate::{app::Key, approval::escape_hiding};
use serde_json::Value;
use std::collections::HashMap;

/// How a piece of inbox text looks; the terminal maps it to its colours.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ink { Normal, Bold, Dim, Heading, Attention, Blue, Suggests, Project(u8), Key, PrimaryKey, Current }

/// One screen line: either text that wraps, or key caps that flow and wrap between keys.
#[derive(Clone, Debug)]
pub enum Line { Text(Vec<(String, Ink)>), Keys(Vec<(String, String, bool)>), Blank }

pub struct Step { pub text: String, pub help: Option<String>, pub message: String }

fn plain(value: &Value) -> Option<String> {
    value.as_str().map(|text|escape_hiding(text.trim(),"","")).filter(|text|!text.is_empty())
}

/// The Chief's steps when it gave them (the last one's `message` is sent); otherwise its suggestion as one step.
pub fn steps(decision: &Value) -> Vec<Step> {
    let given: Vec<Step> = decision["steps"].as_array().into_iter().flatten().filter_map(|step| {
        let text = plain(&step["text"])?;
        Some(Step { message: plain(&step["message"]).unwrap_or_else(||text.clone()), help: plain(&step["help"]), text })
    }).collect();
    if !given.is_empty() { return given; }
    plain(&decision["suggested_answer"]).map(|suggestion|vec![Step { text: suggestion.clone(), help: None, message: suggestion }]).unwrap_or_default()
}

/// The Chief's short situation when it gave one; otherwise its reason for escalating.
pub fn situation(decision: &Value) -> String {
    plain(&decision["situation"]).or_else(||plain(&decision["recommendation"])).unwrap_or_default()
}

/// Whether Theo put this decision off with Later and it has not come back yet.
pub fn is_put_off(record: &Value, now_ms: i64) -> bool {
    record["later_until_ms"].as_i64().is_some_and(|until|until>now_ms)
}
fn open_escalations(snapshot: &Value) -> impl Iterator<Item = &Value> {
    snapshot["records"].as_array().into_iter().flatten()
        .filter(|record|record["kind"]=="decision" && record["state"]=="open" && record["stop_id"].is_string())
}
/// Open escalated questions that are not put off, oldest first.
pub fn questions(snapshot: &Value, now_ms: i64) -> Vec<Value> {
    let mut open: Vec<Value> = open_escalations(snapshot).filter(|record|!is_put_off(record,now_ms)).cloned().collect();
    open.sort_by_key(|record|(record["created_at_ms"].as_i64().unwrap_or(0),record["id"].as_str().unwrap_or("").to_string()));
    open
}
/// Open escalated questions put off with Later, apart from `except` (the one on screen).
pub fn put_off_count(snapshot: &Value, now_ms: i64, except: Option<&str>) -> usize {
    open_escalations(snapshot).filter(|record|is_put_off(record,now_ms) && record["id"].as_str()!=except).count()
}
/// ` · N later` when questions are put off, otherwise nothing.
pub fn later_suffix(count: usize) -> String {
    if count==0 {String::new()} else {format!(" · {count} later")}
}

fn category_words(category: &Value) -> String {
    match category.as_str() {
        Some("not_sure")=>"Not sure".into(), Some("security")=>"Security".into(), Some("scope")=>"Scope".into(),
        Some(other)=>escape_hiding(&other.replace('_'," "),"",""), None=>"Question".into(),
    }
}

fn age(created_ms: Option<i64>, now_ms: i64) -> String {
    let Some(created)=created_ms else {return "asked a while ago".into()};
    let minutes=(now_ms-created).max(0)/60_000;
    match minutes {0=>"asked just now".into(),1..=59=>format!("asked {minutes}m ago"),60..=1439=>format!("asked {}h ago",minutes/60),_=>format!("asked {}d ago",minutes/1440)}
}

/// `project`, `task` for the session a question came from: the host names it `<task> · <project>`.
fn place(snapshot: &Value, decision: &Value) -> (String, String) {
    let session=decision["session_id"].as_str().unwrap_or("");
    match snapshot["session_names"][session].as_str() {
        Some(name)=>match name.rsplit_once(" · ") {
            Some((task,project))=>(escape_hiding(project,"",""),escape_hiding(task,"","")),
            None=>(String::new(),escape_hiding(name,"","")),
        },
        None=>(String::new(),super::view::fallback_session_name(session)),
    }
}
fn place_text(place: &(String, String)) -> String {
    if place.0.is_empty() {place.1.clone()} else {format!("{} › {}",place.0,place.1)}
}

#[derive(Clone, PartialEq, Debug)]
pub enum Stage { Question, Step, Write { text: String, from_last: bool }, Sent { to: String }, Empty }

/// What a key asks the Chief view to do.
pub enum Outcome { None, Close, Send(crate::chief::ChiefHostAction), Later(crate::chief::ChiefHostAction), Details(Value), Answers, Habits }

pub struct Inbox {
    decision: Option<String>,
    /// The revision Theo read; a send carries it, and a change sends him back to read again.
    read_revision: Option<i64>,
    pub stage: Stage,
    notice: Option<String>,
    /// Theo's own ticks per question, kept until it is answered or gone.
    done: HashMap<String, usize>,
    /// What Theo typed for each question: never dropped because the question changed.
    drafts: HashMap<String, String>,
}

const ANSWER_LIMIT: usize = 4000;

impl Inbox {
    pub fn new(snapshot: &Value, now_ms: i64) -> Self {
        let mut inbox=Self {decision:None,read_revision:None,stage:Stage::Empty,notice:None,done:HashMap::new(),drafts:HashMap::new()};
        inbox.show_first(snapshot,now_ms);
        inbox
    }
    fn show_first(&mut self, snapshot: &Value, now_ms: i64) {
        match questions(snapshot,now_ms).first() {
            Some(first)=>{self.decision=first["id"].as_str().map(str::to_string);self.read_revision=first["revision"].as_i64();self.stage=Stage::Question;},
            None=>{self.decision=None;self.read_revision=None;self.stage=Stage::Empty;},
        }
    }
    /// Shows this open question first instead of the oldest; false when it is not one.
    pub fn show(&mut self, snapshot: &Value, now_ms: i64, id: &str) -> bool {
        let Some(question)=questions(snapshot,now_ms).into_iter().find(|question|question["id"].as_str()==Some(id)) else {return false};
        self.decision=Some(id.to_string());self.read_revision=question["revision"].as_i64();self.stage=Stage::Question;
        true
    }
    fn current<'a>(&self, snapshot: &'a Value) -> Option<&'a Value> {
        let id=self.decision.as_deref()?;
        snapshot["records"].as_array()?.iter().find(|record|record["kind"]=="decision" && record["id"].as_str()==Some(id))
    }
    fn draft(&self) -> Option<&String> { self.decision.as_ref().and_then(|id|self.drafts.get(id)).filter(|text|!text.trim().is_empty()) }
    fn keep_draft(&mut self, text: &str) { if let Some(id)=&self.decision {self.drafts.insert(id.clone(),text.to_string());} }
    fn done_count(&self) -> usize { self.decision.as_ref().and_then(|id|self.done.get(id)).copied().unwrap_or(0) }

    /// The host's view changed: a question answered elsewhere or changed sends Theo back to read.
    pub fn refresh(&mut self, snapshot: &Value, now_ms: i64) {
        // "Nothing needs you" must not stay up once something does.
        if self.stage==Stage::Empty {self.show_first(snapshot,now_ms);return;}
        if matches!(self.stage,Stage::Sent {..}) {return;}
        match self.current(snapshot) {
            Some(record) if record["state"]=="open" && record["revision"].as_i64()==self.read_revision=>{},
            Some(record) if record["state"]=="open"=>{
                if let Some(id)=&self.decision {self.done.remove(id);}
                self.read_revision=record["revision"].as_i64();self.stage=Stage::Question;
                self.notice=Some(if self.draft().is_some() {"The question changed; your draft is kept below."} else {"This question changed. Read it again."}.into());
            },
            _=>{
                if let Some(id)=self.decision.take() {self.done.remove(&id);}
                self.show_first(snapshot,now_ms);
                self.notice=Some("That question was answered or withdrawn elsewhere.".into());
            },
        }
    }
    /// The host recorded the answer.
    pub fn sent(&mut self, snapshot: &Value) {
        let to=self.current(snapshot).map(|record|place_text(&place(snapshot,record))).unwrap_or_default();
        if let Some(id)=&self.decision {self.done.remove(id);self.drafts.remove(id);}
        self.stage=Stage::Sent {to};self.notice=None;
    }
    /// The question is put off: show the next one, or nothing.
    pub fn later_done(&mut self, snapshot: &Value, now_ms: i64) { self.show_first(snapshot,now_ms); self.notice=None; }
    pub fn failed(&mut self, message: &str) { self.notice=Some(escape_hiding(message,"","")); }
    pub fn awaits_send(&self, key: &Key) -> bool {
        match (&self.stage,key) {
            (Stage::Step,Key::Char('y'))=>true,
            (Stage::Write {text,..},Key::Ctrl('s'))=>!text.trim().is_empty(),
            (Stage::Question,Key::Char('l'))=>true,
            _=>false,
        }
    }

    fn answer(&self, decision: &Value, text: String) -> crate::chief::ChiefHostAction {
        crate::chief::ChiefHostAction::AnswerDecision {request_id:format!("chief-ui-{}",crate::util::token_hex(12)),
            decision:decision["id"].as_str().unwrap_or("").into(),expected_revision:self.read_revision.unwrap_or(0),answer:text}
    }

    /// A notice stays until a key changes something, so a key that does nothing never hides why.
    pub fn handle(&mut self, key: Key, snapshot: &Value, now_ms: i64) -> Outcome {
        let (notice,stage)=(self.notice.take(),self.stage.clone());
        let outcome=self.act(key,snapshot,now_ms);
        if matches!(outcome,Outcome::None) && self.stage==stage && self.notice.is_none() {self.notice=notice;}
        outcome
    }
    fn act(&mut self, key: Key, snapshot: &Value, now_ms: i64) -> Outcome {
        let record=self.current(snapshot).cloned();
        let steps=record.as_ref().map(steps).unwrap_or_default();
        let last=steps.len().saturating_sub(1);
        match self.stage.clone() {
            Stage::Question=>match (key,record) {
                (Key::Char('a'),Some(_)) if !steps.is_empty()=>self.stage=Stage::Step,
                (Key::Char('w'),Some(_))=>self.stage=Stage::Write {text:self.draft().cloned().unwrap_or_default(),from_last:false},
                (Key::Char('l'),Some(record))=>return Outcome::Later(crate::chief::ChiefHostAction::LaterDecision {
                    request_id:format!("chief-ui-{}",crate::util::token_hex(12)),decision:record["id"].as_str().unwrap_or("").into(),
                    expected_revision:self.read_revision.unwrap_or(0)}),
                (Key::Char('i'),Some(record))=>return Outcome::Details(record),
                (Key::Esc,_)=>return Outcome::Close,
                _=>{},
            },
            Stage::Step=>{
                let done=self.done_count();
                match key {
                    Key::Char('d') if done<last=>{if let Some(id)=&self.decision {self.done.insert(id.clone(),done+1);}},
                    Key::Char('y') if done==last=>if let (Some(record),Some(step))=(record,steps.last()) {
                        let action=self.answer(&record,step.message.clone());
                        if action.validate().is_ok() {return Outcome::Send(action);}
                    },
                    Key::Char('e') if done==last=>if let Some(step)=steps.last() {self.stage=Stage::Write {text:step.message.clone(),from_last:true};},
                    Key::Esc=>self.stage=Stage::Question,
                    _=>{},
                }
            },
            Stage::Write {mut text,from_last}=>match key {
                Key::Ctrl('s') if !text.trim().is_empty()=>if let Some(record)=record {
                    let action=self.answer(&record,text);
                    match action.validate() {Ok(())=>return Outcome::Send(action),Err(error)=>self.notice=Some(error.message)}
                },
                Key::Char(character) if !character.is_control()=>{if text.chars().count()<ANSWER_LIMIT {text.push(character);} self.keep_draft(&text);self.stage=Stage::Write {text,from_last};},
                Key::Paste(paste)=>{for character in paste.chars().filter(|character|*character=='\n' || !character.is_control()) {if text.chars().count()<ANSWER_LIMIT {text.push(character);}} self.keep_draft(&text);self.stage=Stage::Write {text,from_last};},
                Key::Enter=>{if text.chars().count()<ANSWER_LIMIT {text.push('\n');} self.keep_draft(&text);self.stage=Stage::Write {text,from_last};},
                Key::Backspace=>{text.pop();self.keep_draft(&text);self.stage=Stage::Write {text,from_last};},
                // Esc from edit first returns to the message unchanged; Esc from write my own keeps the draft.
                Key::Esc=>{if from_last {if let Some(id)=&self.decision {self.drafts.remove(id);}} self.stage=if from_last {Stage::Step} else {Stage::Question};},
                _=>{},
            },
            Stage::Sent {..}=>match key {
                Key::Char('n')=>self.show_first(snapshot,now_ms),
                Key::Esc=>return Outcome::Close,
                _=>{},
            },
            Stage::Empty=>match key {
                Key::Char('l')=>return Outcome::Answers,
                Key::Char('h')=>return Outcome::Habits,
                Key::Esc=>return Outcome::Close,
                _=>{},
            },
        }
        Outcome::None
    }

    fn header(&self, snapshot: &Value, now_ms: i64, lines: &mut Vec<Line>) {
        let open=questions(snapshot,now_ms);
        let current=self.decision.as_deref();
        // A question put off elsewhere while it is on screen still counts here, after the open ones.
        let (position,total)=match open.iter().position(|record|record["id"].as_str()==current) {
            Some(index)=>(index,open.len()),
            None=>(open.len(),open.len()+1),
        };
        let later=later_suffix(put_off_count(snapshot,now_ms,current));
        let dots: String=(0..total.min(8)).map(|index|if index==position {'●'} else {'○'}).collect();
        lines.push(Line::Text(vec![(format!("! Chief needs you · {} of {total}{later}",position+1),Ink::Attention),(format!("  {dots}"),Ink::Dim)]));
    }
    fn context(snapshot: &Value, record: &Value, now_ms: i64, lines: &mut Vec<Line>) {
        let (project,task)=place(snapshot,record);
        let mut context=Vec::new();
        if !project.is_empty() {context.push((project.clone(),Ink::Project(crate::app::project_color(&project))));context.push((" › ".into(),Ink::Dim));}
        context.push((task,Ink::Normal));
        lines.push(Line::Text(context));
        lines.push(Line::Text(vec![(format!("{} · {}",category_words(&record["category"]),age(record["created_at_ms"].as_i64(),now_ms)),Ink::Dim)]));
    }

    /// The card for the current stage, top to bottom.
    pub fn lines(&self, snapshot: &Value, now_ms: i64) -> Vec<Line> {
        let mut lines=Vec::new();
        let record=self.current(snapshot);
        if let Some(notice)=&self.notice {lines.push(Line::Text(vec![(notice.clone(),Ink::Attention)]));lines.push(Line::Blank);}
        match (&self.stage,record) {
            (Stage::Empty,_)=>{
                lines.push(Line::Text(vec![("✓ Nothing needs you".into(),Ink::Blue)]));
                if let Some(answered)=snapshot["today"]["answered"].as_u64() {
                    lines.push(Line::Text(vec![(format!("The Chief answered {answered} question{} today.",if answered==1 {""} else {"s"}),Ink::Dim)]));
                }
                lines.push(Line::Keys(vec![("l".into(),"what it answered".into(),false),("h".into(),"habits".into(),false),("esc".into(),"back".into(),false)]));
            },
            (Stage::Sent {to},_)=>{
                lines.push(Line::Text(vec![(format!("✓ Sent to {to}"),Ink::Blue)]));
                lines.push(Line::Blank);
                let open=questions(snapshot,now_ms);
                match open.first() {
                    Some(next)=>{
                        lines.push(Line::Text(vec![(format!("NEXT · 1 OF {}",open.len()),Ink::Attention)]));
                        let (project,task)=place(snapshot,next);
                        let mut context=Vec::new();
                        if !project.is_empty() {context.push((project.clone(),Ink::Project(crate::app::project_color(&project))));context.push((" › ".into(),Ink::Dim));}
                        context.push((task,Ink::Normal));
                        lines.push(Line::Text(context));
                        lines.push(Line::Text(vec![(plain(&next["question"]).unwrap_or_default(),Ink::Normal)]));
                        lines.push(Line::Keys(vec![("n".into(),"next question".into(),true),("esc".into(),"stop here".into(),false)]));
                    },
                    None=>{
                        lines.push(Line::Text(vec![("Nothing else needs you.".into(),Ink::Dim)]));
                        lines.push(Line::Keys(vec![("n".into(),"done".into(),true),("esc".into(),"stop here".into(),false)]));
                    },
                }
                lines.push(Line::Text(vec![("Never jumps by itself. The rest wait here and on their session cards.".into(),Ink::Dim)]));
            },
            (_,None)=>lines.push(Line::Text(vec![("This question is no longer available.".into(),Ink::Dim)])),
            (Stage::Question,Some(record))=>{
                self.header(snapshot,now_ms,&mut lines);
                Self::context(snapshot,record,now_ms,&mut lines);
                lines.push(Line::Blank);
                lines.push(Line::Text(vec![("WHAT'S GOING ON".into(),Ink::Heading)]));
                lines.push(Line::Text(vec![(situation(record),Ink::Normal)]));
                lines.push(Line::Blank);
                let steps=steps(record);
                let mut keys=Vec::new();
                if steps.is_empty() {
                    lines.push(Line::Text(vec![("The Chief suggests no answer.".into(),Ink::Dim)]));
                } else {
                    lines.push(Line::Text(vec![(format!("◆ CHIEF SUGGESTS · {} step{}",steps.len(),if steps.len()==1 {""} else {"s"}),Ink::Suggests)]));
                    for (index,step) in steps.iter().enumerate() {lines.push(Line::Text(vec![(format!("{}  {}",index+1,step.text),Ink::Normal)]));}
                    keys.push(("a".into(),"do this, step by step".into(),true));
                }
                if let Some(draft)=self.draft() {
                    lines.push(Line::Blank);
                    lines.push(Line::Text(vec![("YOUR DRAFT".into(),Ink::Heading)]));
                    for line in escape_hiding(draft,"","").split('\n') {lines.push(Line::Text(vec![(line.to_string(),Ink::Current)]));}
                }
                lines.push(Line::Blank);
                keys.extend([("w".into(),if self.draft().is_some() {"continue my answer"} else {"write my own"}.into(),false),("l".into(),"later".into(),false),("i".into(),"full details".into(),false)]);
                lines.push(Line::Keys(keys));
                lines.push(Line::Blank);
                lines.push(Line::Text(vec![("Written by the Chief: check it before acting.".into(),Ink::Dim)]));
            },
            (Stage::Step,Some(record))=>{
                self.header(snapshot,now_ms,&mut lines);
                Self::context(snapshot,record,now_ms,&mut lines);
                lines.push(Line::Blank);
                let steps=steps(record);
                let done=self.done_count().min(steps.len().saturating_sub(1));
                for step in &steps[..done] {lines.push(Line::Text(vec![(format!("✓ {}",step.text),Ink::Dim)]));}
                let current=&steps[done];
                if done+1==steps.len() {
                    lines.push(Line::Text(vec![(format!("STEP {} OF {} · SEND TO THE AGENT",done+1,steps.len()),Ink::Blue)]));
                    lines.push(Line::Text(vec![(current.message.clone(),Ink::Current)]));
                    lines.push(Line::Keys(vec![("y".into(),"send".into(),true),("e".into(),"edit first".into(),false)]));
                    lines.push(Line::Blank);
                    lines.push(Line::Text(vec![("Enter never sends.".into(),Ink::Dim)]));
                } else {
                    lines.push(Line::Text(vec![(format!("STEP {} OF {}",done+1,steps.len()),Ink::Blue)]));
                    lines.push(Line::Text(vec![(current.text.clone(),Ink::Bold)]));
                    if let Some(help)=&current.help {lines.push(Line::Text(vec![(help.clone(),Ink::Dim)]));}
                    lines.push(Line::Keys(vec![("d".into(),"done, next step".into(),true)]));
                    lines.push(Line::Blank);
                    lines.push(Line::Text(vec![("THEN".into(),Ink::Heading)]));
                    for (index,step) in steps.iter().enumerate().skip(done+1) {
                        let text=if index+1==steps.len() {format!("{}  Send to the agent: “{}”",index+1,step.message)} else {format!("{}  {}",index+1,step.text)};
                        lines.push(Line::Text(vec![(text,Ink::Dim)]));
                    }
                    lines.push(Line::Blank);
                    lines.push(Line::Keys(vec![("esc".into(),"back to the question".into(),false)]));
                }
            },
            (Stage::Write {text,..},Some(record))=>{
                self.header(snapshot,now_ms,&mut lines);
                Self::context(snapshot,record,now_ms,&mut lines);
                lines.push(Line::Blank);
                lines.push(Line::Text(vec![("YOUR ANSWER".into(),Ink::Heading)]));
                for line in format!("{}▏",escape_hiding(text,"","")).split('\n') {lines.push(Line::Text(vec![(line.to_string(),Ink::Current)]));}
                lines.push(Line::Blank);
                lines.push(Line::Text(vec![(format!("Goes to {}, exactly as written.",place_text(&place(snapshot,record))),Ink::Dim)]));
                lines.push(Line::Keys(vec![("⌘S".into(),"send".into(),true),("esc".into(),"back".into(),false)]));
            },
        }
        lines
    }
}
