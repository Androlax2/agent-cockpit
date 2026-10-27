//! Host IPC and existing terminal rendering for Chief; no store access or command runner.
use super::*;
use serde_json::json;
use crate::{chief::{host_snapshot as snapshot, ChiefHostAction, view::{ChiefView, Effect, ListLine, Mark, Tone}}, teams::HostRequest};

impl App {
    pub(super) fn refresh_chief_status(&mut self) {
        self.chief_status = snapshot().map_err(|error| error.message);
    }
    pub(super) fn open_chief_view(&mut self) -> String {
        if let Some(view)=self.chief_view.as_mut() {view.suspended=false;self.refresh_chief_view();return String::new();}
        match snapshot() {
            Ok(snapshot)=>{let mut view=ChiefView::new(snapshot);view.habits_seen_ms=crate::chief::habits_seen_ms();view.open_inbox();self.chief_view=Some(view);String::new()},
            Err(error)=>{
                let mut view=ChiefView::new(json!({"records":[]}));
                view.read_failed(format!("Chief view unavailable: {}",error.message));
                view.results(format!("Host review unavailable: {}. No state or permission is inferred. Esc returns without any action.",crate::approval::escape_hiding(&error.message,"","")));
                self.chief_view=Some(view);String::new()
            },
        }
    }
    /// Opens the Chief view on its answers list, for one session or for all of them.
    pub(super) fn open_chief_answers(&mut self, session_id: Option<&str>) -> String {
        let message=self.open_chief_view();
        let card_count=session_id.and_then(|id|self.chief_answers.get(id)).map_or(0,|answers|answers.count);
        if let Some(view)=self.chief_view.as_mut() {view.open_answers(session_id.map(str::to_string));view.card_count=card_count;}
        message
    }
    pub(super) fn refresh_chief_view(&mut self) -> bool {
        match snapshot() {
            // A digest that showed the habits meanwhile has saved that Theo saw them.
            Ok(snapshot)=>{if let Some(view)=self.chief_view.as_mut() {view.refresh(snapshot);view.habits_seen_ms=view.habits_seen_ms.max(crate::chief::habits_seen_ms());}true},
            Err(error)=>{if let Some(view)=self.chief_view.as_mut() {view.read_failed(format!("Host review unavailable: {}",error.message));}false},
        }
    }
    pub(super) fn handle_chief_key(&mut self, key: Key) -> bool {
        let Some(view)=self.chief_view.as_mut() else {return false};
        if matches!(key,Key::Tab|Key::BackTab) {
            view.suspend();
            self.focus_panel(if key==Key::Tab {if self.focus==0 {1} else {self.focus%(PANEL_NAMES.len()-1)+1}} else if self.focus<=1 {PANEL_NAMES.len()-1} else {self.focus-1});
            return true;
        }
        if view.suspended {return false;}
        if view.submission_pending(&key) && !self.refresh_chief_view() {return true;}
        let effect=self.chief_view.as_mut().expect("Chief is open").handle(key);
        match effect {
            Effect::InboxSend(action)=> {
                let result=crate::teams::client::host_request(&HostRequest::Chief {action}).and_then(|reply|reply.result);
                self.refresh_chief_view();
                let view=self.chief_view.as_mut().expect("Chief is open");
                match result {Ok(_)=>view.inbox_sent(),Err(error)=>view.inbox_failed(&format!("Not sent: {}. It may have arrived; check before sending again.",error.message))}
            },
            Effect::InboxLater(action)=> {
                let result=crate::teams::client::host_request(&HostRequest::Chief {action}).and_then(|reply|reply.result);
                self.refresh_chief_view();
                let view=self.chief_view.as_mut().expect("Chief is open");
                match result {Ok(_)=>view.inbox_later_done(),Err(error)=>view.inbox_failed(&format!("Not put off: {}",error.message))}
            },
            Effect::None=>{},Effect::Close=> {
                if !self.chief_view.as_mut().expect("Chief is open").suspend_pending_start() {self.chief_view=None;}
            },
            Effect::Start(action)=> {
                let result=crate::teams::client::host_request(&HostRequest::Chief {action}).and_then(|reply|reply.result);
                let view=self.chief_view.as_mut().expect("Chief is open");
                match result {Ok(_)=>view.startup_acknowledged(),Err(error)=>view.startup_failed(error.message)}
                self.refresh_chief_view();
            },
            Effect::SendCheck(action)=> {
                let result=crate::teams::client::host_request(&HostRequest::Chief {action}).and_then(|reply|reply.result);
                self.refresh_chief_view();
                if let Err(error)=result {self.chief_view.as_mut().expect("Chief is open").check_failed(format!("Test message not sent: {}",error.message));}
            },
            Effect::ConfirmCheck(session,evidence)=> {
                let request=HostRequest::VerifyChiefAdmission {session,adapter:crate::chief::ADMISSION_ADAPTER.into(),evidence};
                let result=crate::teams::client::host_request(&request).and_then(|reply|reply.result);
                self.refresh_chief_view();
                if let Err(error)=result {self.chief_view.as_mut().expect("Chief is open").check_failed(format!("Not recorded: {}. Press y again once you are sure.",error.message));}
            },
            Effect::Summary=> {
                let action=ChiefHostAction::RequestSummary {request_id:format!("chief-ui-{}",util::token_hex(12))};
                let result=crate::teams::client::host_request(&HostRequest::Chief {action}).and_then(|reply|reply.result);
                self.refresh_chief_view();
                if let Err(error)=result {self.chief_view.as_mut().expect("Chief is open").fail(format!("Summary request failed/uncertain: {}. No automatic retry.",error.message));}
            },
            Effect::HabitsSeen(seen_ms)=>if let Err(error)=crate::chief::save_habits_seen(seen_ms) {
                self.chief_view.as_mut().expect("Chief is open").fail(format!("Cannot save that you saw your habits: {error}. They show as new again next time."));
            },
            Effect::Change(action)=> {
                let result=crate::teams::client::host_request(&HostRequest::Chief {action}).and_then(|reply|reply.result);
                self.refresh_chief_view();
                if let Err(error)=result {self.chief_view.as_mut().expect("Chief is open").habit_change_failed(format!("Habit not changed: {}",error.message));}
            },
            Effect::Submit(actions)=> {
                let mut results=Vec::new();let mut failed=false;
                for action in actions {
                    let label=crate::chief::view::shown(&serde_json::to_value(&action).expect("action serializes"));
                    if failed {results.push(format!("UNSENT / unapproved: {label}"));continue;}
                    match crate::teams::client::host_request(&HostRequest::Chief {action}).and_then(|reply|reply.result) {
                        Ok(result)=>results.push(format!("Host result: {} · {label}",crate::chief::view::shown(&result))),
                        Err(error)=>{results.push(format!("FAILED / outcome may be uncertain: {} · {label}. Never blindly retry.",crate::approval::escape_hiding(&error.message,"","")));failed=true;},
                    }
                }
                self.refresh_chief_view();
                self.chief_view.as_mut().expect("Chief is open").results(results.join("\n\n"));
            },
        }
        true
    }
}

fn list_style(line: &ListLine) -> Style {
    match (line.tone,line.selected) {
        (_,true)=>on(FG,BG_HIGHLIGHT),
        (Tone::Dim,false)=>dim(FG_DARK),
        (Tone::Normal,false)=>style(FG),
    }
}
fn mark_style(line: &ListLine, mark: Mark) -> Style {
    let (color,is_bold)=match mark {Mark::Alert=>(RED,false),Mark::Attention=>(YELLOW,true),Mark::Learned=>(MAGENTA,true),Mark::Project(color)=>(color,false)};
    let marked=if line.selected {on(color,BG_HIGHLIGHT)} else {style(color)};
    if is_bold {marked.add_modifier(Modifier::BOLD)} else {marked}
}

pub(super) fn draw(view: &mut ChiefView, buffer: &mut Buffer) {
    let (width,height)=(buffer.area.width,buffer.area.height);
    if width<8 || height<8 {return;}
    if view.startup_screen() {draw_startup(view,buffer);return;}
    if view.inbox_active() {draw_inbox(view,buffer);return;}
    let (top,bottom)=(HEADER_LINES,height-2);let inner=width.saturating_sub(4) as usize;let body=inner.saturating_sub(2).max(1);
    for row in top..=bottom {
        put(buffer,row,1,&" ".repeat((width-2) as usize),style(FG));
        put(buffer,row,1,"│",style(BLUE));put(buffer,row,width-2,"│",style(BLUE));
    }
    put(buffer,top,1,&format!("┌{}┐","─".repeat(inner)),style(BLUE));
    let is_list=view.is_list();
    let footer_text=if width<30 {view.compact_footer()} else {view.footer()};
    let footer:Vec<_>=footer_text.lines().flat_map(|line|wrap_detail_text(line,body)).take(bottom.saturating_sub(top+2) as usize).collect();
    let footer_start=bottom.saturating_sub(footer.len() as u16);
    let available=footer_start.saturating_sub(top+1) as usize;
    let (title,lines):(String,Vec<(String,Style,Vec<(usize,usize,Style)>)>)=if is_list {
        let (title,lines)=view.list_window(available);
        (title,lines.iter().map(|line|(fit(&line.text,body),list_style(line),line.marks.iter().map(|(start,end,mark)|(*start,*end,mark_style(line,*mark))).collect())).collect())
    } else {("Chief · global coordination".into(),view.detail().iter().flat_map(|line|wrap_detail_text(line,body)).map(|line|(line,style(FG),Vec::new())).collect())};
    put(buffer,top,3,&fit(&title,body),bold(BLUE));
    view.page_step=available.max(1).min(5);
    let first=view.clamp_scroll(lines.len().saturating_sub(available));
    for (offset,(line,line_style,marks)) in lines.iter().skip(first).take(available).enumerate() {
        let row=top+1+offset as u16;
        put(buffer,row,3,line,*line_style);
        let characters:Vec<char>=line.chars().collect();
        for (start,end,mark) in marks {
            let (start,end)=((*start).min(characters.len()),(*end).min(characters.len()));
            if start<end {
                let before:String=characters[..start].iter().collect();let word:String=characters[start..end].iter().collect();
                put(buffer,row,3+width_of(&before) as u16,&word,*mark);
            }
        }
    }
    for (offset,line) in footer.iter().enumerate() {put(buffer,footer_start+offset as u16,3,line,dim(FG_DARK));}
    put(buffer,bottom,1,&format!("└{}┘","─".repeat(inner)),style(BLUE));
}

fn draw_startup(view: &mut ChiefView, buffer: &mut Buffer) {
    let (width,height)=(buffer.area.width,buffer.area.height);
    let body=width.saturating_sub(6) as usize;
    let top=HEADER_LINES;
    for row in top..height-1 {put(buffer,row,0,&" ".repeat(width as usize),style(FG));}
    put(buffer,top,2,"‹ Status",style(BLUE));
    let mut lines=Vec::new();
    for (index,line) in view.startup_lines().iter().enumerate() {
        let appearance=if is_startup_action(line) {on(BLACK,BLUE)}
            else if index==0 {bold(match view.startup_phase() {"failed"|"unavailable"=>RED,"off"=>FG,_=>BLUE})}
            else if line.starts_with("You can leave") || line.starts_with("Creates its own") || line.starts_with("No session remains") || line=="Starting…" {dim(FG_DARK)}
            else {style(FG)};
        lines.extend(wrap_detail_text(line,body).into_iter().map(|text|(text,appearance)));
    }
    let available=height.saturating_sub(top+5) as usize;
    view.page_step=available.max(1).min(5);
    let first=view.clamp_scroll(lines.len().saturating_sub(available));
    for (offset,(line,appearance)) in lines.iter().skip(first).take(available).enumerate() {
        let row=top+2+offset as u16;
        if is_startup_action(line) {put(buffer,row,3,&" ".repeat(body),*appearance);}
        put(buffer,row,3,line,*appearance);
        if is_startup_action(line) {put(buffer,row,3,&line[..3],on(FG,BLACK));}
    }
    put(buffer,height-3,2,&fit(view.footer(),body),dim(FG_DARK));
}

/// A `[k] Label` line: one action and the key cap that activates it.
fn is_startup_action(line: &str) -> bool {
    let bytes=line.as_bytes();
    bytes.len()>4 && bytes[0]==b'[' && bytes[1].is_ascii_lowercase() && bytes[2]==b']' && bytes[3]==b' '
}

fn ink_style(ink: crate::chief::inbox::Ink) -> Style {
    use crate::chief::inbox::Ink;
    match ink {
        Ink::Normal=>style(FG), Ink::Bold=>bold(FG), Ink::Dim=>dim(FG_DARK), Ink::Heading=>bold(FG_DARK),
        Ink::Attention=>bold(YELLOW), Ink::Blue=>bold(BLUE), Ink::Suggests=>bold(MAGENTA), Ink::Project(color)=>bold(color),
        Ink::Key=>on(FG,BG_HIGHLIGHT), Ink::PrimaryKey=>on(BLACK,YELLOW), Ink::Current=>style(FG),
    }
}

/// The inbox card (frames 3097-3222): wrapped text, key caps that wrap between keys, the current step outlined in blue.
fn draw_inbox(view: &mut ChiefView, buffer: &mut Buffer) {
    use crate::chief::inbox::{Ink, Line};
    let (width,height)=(buffer.area.width,buffer.area.height);
    let top=HEADER_LINES;
    for row in top..height-1 {put(buffer,row,0,&" ".repeat(width as usize),style(FG));}
    put(buffer,top,2,"‹ Chief",style(BLUE));
    let body=width.saturating_sub(6).max(8) as usize;
    let mut rows: Vec<Vec<(String,Style)>>=Vec::new();
    for line in view.inbox_lines() {
        match line {
            Line::Blank=>rows.push(Vec::new()),
            Line::Text(segments) if segments.len()==1 && segments[0].1==Ink::Current=>{
                for text in wrap_detail_text(&segments[0].0,body.saturating_sub(2)) {rows.push(vec![("▎ ".into(),style(BLUE)),(text,style(FG))]);}
            },
            Line::Text(segments) if segments.len()==1=>{
                let ink=ink_style(segments[0].1);
                for text in wrap_detail_text(&segments[0].0,body) {rows.push(vec![(text,ink)]);}
            },
            Line::Text(segments)=>{
                let joined: String=segments.iter().map(|(text,_)|text.as_str()).collect();
                if width_of(&joined)<=body {rows.push(segments.into_iter().map(|(text,ink)|(text,ink_style(ink))).collect());}
                else {for text in wrap_detail_text(&joined,body) {rows.push(vec![(text,ink_style(segments[0].1))]);}}
            },
            Line::Keys(keys)=>{
                let mut row: Vec<(String,Style)>=Vec::new();let mut used=0;
                for (cap,label,primary) in keys {
                    let item=width_of(&cap)+2+1+width_of(&label)+2;
                    if used>0 && used+item>body {rows.push(std::mem::take(&mut row));used=0;}
                    row.push((format!(" {cap} "),ink_style(if primary {Ink::PrimaryKey} else {Ink::Key})));
                    row.push((format!(" {label}  "),dim(FG_DARK)));
                    used+=item;
                }
                if !row.is_empty() {rows.push(row);}
            },
        }
    }
    let available=height.saturating_sub(top+3) as usize;
    view.page_step=available.max(1).min(5);
    let first=view.clamp_scroll(rows.len().saturating_sub(available));
    for (offset,segments) in rows.iter().skip(first).take(available).enumerate() {
        let row=top+2+offset as u16;let mut column=3u16;
        for (text,appearance) in segments {
            let room=(width as usize).saturating_sub(column as usize+1);
            let shown=fit(text,room);
            put(buffer,row,column,&shown,*appearance);
            column+=width_of(&shown) as u16;
        }
    }
}
