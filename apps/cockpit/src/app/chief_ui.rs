//! Host IPC and existing terminal rendering for Chief; no store access or command runner.
use super::*;
use serde_json::json;
use crate::{chief::{ChiefHostAction, view::{ChiefView, Effect}}, teams::{HostRequest, TeamError, ErrorCode}};

fn snapshot() -> Result<Value, TeamError> {
    // A changing generation restarts the complete read, never mixes pages or advances an event cursor.
    for _ in 0..3 {
        let mut records=Vec::new(); let mut after=0; let mut generation=None; let mut envelope=None;
        for _ in 0..512 {
            let page=crate::teams::client::host_request(&HostRequest::ChiefSnapshot {after,limit:50,generation:generation.clone()})?.result?;
            if page["invalidated"]==true {break;}
            if page["projection"]!="host_review" || page["cursor_kind"]!="snapshot_offset" {
                return Err(TeamError::new(ErrorCode::Unavailable,"Chief host review projection is missing"));
            }
            let current=page["generation"].as_str().ok_or_else(||TeamError::new(ErrorCode::Unavailable,"Chief snapshot generation is missing"))?.to_string();
            if generation.as_ref().is_some_and(|generation|generation!=&current) {break;}
            generation=Some(current);
            let batch=page["records"].as_array().ok_or_else(||TeamError::new(ErrorCode::Unavailable,"Chief snapshot records are missing"))?;
            records.extend(batch.iter().cloned());
            if envelope.is_none() {envelope=Some(page.clone());}
            if page["has_more"]==false {
                let mut complete=envelope.expect("first page is present");complete["records"]=json!(records);complete["has_more"]=json!(false);return Ok(complete);
            }
            let next=page["next"].as_i64().filter(|next|*next>after).ok_or_else(||TeamError::new(ErrorCode::Unavailable,"Chief snapshot cursor did not advance"))?;
            after=next;
        }
    }
    Err(TeamError::new(ErrorCode::Unavailable,"Chief host snapshot changed repeatedly or exceeded its bounded read; try opening again"))
}
impl App {
    pub(super) fn open_chief_view(&mut self) -> String {
        if let Some(view)=self.chief_view.as_mut() {view.suspended=false;self.refresh_chief_view();return String::new();}
        match snapshot() {
            Ok(snapshot)=>{self.chief_view=Some(ChiefView::new(snapshot));String::new()},
            Err(error)=>{
                let mut view=ChiefView::new(json!({"records":[]}));
                view.read_failed(format!("Chief view unavailable: {}",error.message));
                view.results(format!("Host review unavailable: {}. No state or permission is inferred. Esc returns without any action.",crate::approval::escape_hiding(&error.message,"","")));
                self.chief_view=Some(view);String::new()
            },
        }
    }
    pub(super) fn refresh_chief_view(&mut self) -> bool {
        match snapshot() {
            Ok(snapshot)=>{if let Some(view)=self.chief_view.as_mut() {view.refresh(snapshot);}true},
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
            Effect::None=>{},Effect::Close=>self.chief_view=None,
            Effect::Summary=> {
                let action=ChiefHostAction::RequestSummary {request_id:format!("chief-ui-{}",util::token_hex(12))};
                let result=crate::teams::client::host_request(&HostRequest::Chief {action}).and_then(|reply|reply.result);
                self.refresh_chief_view();
                if let Err(error)=result {self.chief_view.as_mut().expect("Chief is open").fail(format!("Summary request failed/uncertain: {}. No automatic retry.",error.message));}
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

pub(super) fn draw(view: &mut ChiefView, buffer: &mut Buffer) {
    let (width,height)=(buffer.area.width,buffer.area.height);
    if width<8 || height<8 {return;}
    let (top,bottom)=(HEADER_LINES,height-2);let inner=width.saturating_sub(4) as usize;let body=inner.saturating_sub(2).max(1);
    for row in top..=bottom {
        put(buffer,row,1,&" ".repeat((width-2) as usize),on(FG,BLACK));
        put(buffer,row,1,"│",style(BLUE));put(buffer,row,width-2,"│",style(BLUE));
    }
    put(buffer,top,1,&format!("┌{}┐","─".repeat(inner)),style(BLUE));
    let is_list=view.is_list();
    let footer_text=if width<30 {view.compact_footer()} else {view.footer()};
    let footer:Vec<_>=footer_text.lines().flat_map(|line|wrap_detail_text(line,body)).take(bottom.saturating_sub(top+2) as usize).collect();
    let footer_start=bottom.saturating_sub(footer.len() as u16);
    let available=footer_start.saturating_sub(top+1) as usize;
    let (title,lines)=if is_list {
        let (title,lines)=view.list_window(available);(title,lines.iter().map(|line|fit(line,body)).collect::<Vec<_>>())
    } else {("Chief · global coordination".into(),view.detail().iter().flat_map(|line|wrap_detail_text(line,body)).collect())};
    put(buffer,top,3,&fit(&title,body),bold(BLUE));
    view.page_step=available.max(1).min(5);
    let first=view.clamp_scroll(lines.len().saturating_sub(available));
    for (offset,line) in lines.iter().skip(first).take(available).enumerate() {
        put(buffer,top+1+offset as u16,3,line,if is_list && line.starts_with('>') {on(FG,BG_HIGHLIGHT)} else {style(FG)});
    }
    for (offset,line) in footer.iter().enumerate() {put(buffer,footer_start+offset as u16,3,line,dim(FG_DARK));}
    put(buffer,bottom,1,&format!("└{}┘","─".repeat(inner)),style(BLUE));
}
