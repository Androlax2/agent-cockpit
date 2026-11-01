//! The `b` form (#471): the owner reports a Cockpit bug, `cockpit bug-report new` in the TUI.
//! It only drafts: review, edit and send stay in the CLI (`cockpit bug-report show|send`).

use cockpit_protocol::Command;

use crate::Key;
use crate::form::FormOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BugField {
    Description,
    Title,
    Steps,
}

const FIELDS: [BugField; 3] = [BugField::Description, BugField::Title, BugField::Steps];

#[derive(Debug, Clone)]
pub struct BugForm {
    pub(crate) description: String,
    /// Empty: the daemon titles the draft from its description.
    pub(crate) title: String,
    pub(crate) steps: String,
    pub(crate) field: BugField,
    /// Why the last submit was refused, for the owner.
    pub(crate) problem: Option<String>,
    /// The `bug-report-new` this form sent and the daemon has not answered: Enter sends no
    /// second one, and only its answer closes or annotates the form.
    pub(crate) submitted: Option<Command>,
}

impl Default for BugForm {
    fn default() -> BugForm {
        BugForm::new()
    }
}

impl BugForm {
    pub fn new() -> BugForm {
        BugForm {
            description: String::new(),
            title: String::new(),
            steps: String::new(),
            field: BugField::Description,
            problem: None,
            submitted: None,
        }
    }

    /// While its draft is on its way only Esc acts: an edit made then would be lost when the
    /// answer closes the form, and the draft would not have it.
    pub fn key(&mut self, key: Key) -> FormOutcome {
        if self.submitted.is_some() && key != Key::Esc {
            return FormOutcome::Editing;
        }
        match key {
            Key::Esc => return FormOutcome::Cancelled,
            Key::Enter => return self.submit(),
            Key::Tab | Key::Down => self.field = self.step_field(1),
            Key::BackTab | Key::Up => self.field = self.step_field(FIELDS.len() - 1),
            Key::Backspace => {
                self.text_field().pop();
            }
            Key::Char(character) => self.text_field().push(character),
            Key::Left | Key::Right => {}
        }
        FormOutcome::Editing
    }

    fn step_field(&self, step: usize) -> BugField {
        let at = FIELDS
            .iter()
            .position(|field| *field == self.field)
            .expect("the field is one of the form's");
        FIELDS[(at + step) % FIELDS.len()]
    }

    fn text_field(&mut self) -> &mut String {
        match self.field {
            BugField::Description => &mut self.description,
            BugField::Title => &mut self.title,
            BugField::Steps => &mut self.steps,
        }
    }

    fn submit(&mut self) -> FormOutcome {
        if self.description.trim().is_empty() {
            self.problem = Some("describe what went wrong".to_owned());
            return FormOutcome::Editing;
        }
        let command = Command::BugReportNew {
            description: self.description.trim().to_owned(),
            title: Some(self.title.trim().to_owned()).filter(|title| !title.is_empty()),
            steps: self.steps.trim().to_owned(),
        };
        self.submitted = Some(command.clone());
        self.problem = None;
        FormOutcome::Submit(command)
    }
}
