//! The basic TUI's one form, `N`: a new session. Project (a known project's root, or a typed
//! path), agent (the providers the daemon offers), an optional first message, and in place
//! or in a new parallel clone on a branch.

use cockpit_protocol::{Command, ProviderView};

use crate::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Project,
    Agent,
    Message,
    Place,
    Branch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    InPlace,
    NewClone,
}

/// What a key did to the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormOutcome {
    Editing,
    Cancelled,
    Submit(Command),
}

#[derive(Debug, Clone)]
pub struct Form {
    known_roots: Vec<String>,
    pub(crate) project: String,
    /// `None` until the daemon answered `provider-list`.
    pub(crate) agents: Option<Vec<ProviderView>>,
    pub(crate) agent: usize,
    pub(crate) message: String,
    pub(crate) place: Place,
    pub(crate) branch: String,
    pub(crate) field: Field,
    /// Why the last submit was refused, for the owner.
    pub(crate) problem: Option<String>,
    /// A submitted `session-new` is unanswered: Enter sends no second one.
    pub(crate) starting: bool,
}

impl Form {
    /// A form on the first known project; `known_roots` are the projects' roots.
    pub fn new(known_roots: Vec<String>) -> Form {
        Form {
            project: known_roots.first().cloned().unwrap_or_default(),
            known_roots,
            agents: None,
            agent: 0,
            message: String::new(),
            place: Place::InPlace,
            branch: String::new(),
            field: Field::Project,
            problem: None,
            starting: false,
        }
    }

    pub fn set_agents(&mut self, agents: Vec<ProviderView>) {
        self.agent = 0;
        self.agents = Some(agents);
    }

    pub fn field(&self) -> Field {
        self.field
    }

    pub fn key(&mut self, key: Key) -> FormOutcome {
        match key {
            Key::Esc => return FormOutcome::Cancelled,
            Key::Enter => return self.submit(),
            Key::Tab | Key::Down => self.field = self.next_field(),
            Key::BackTab | Key::Up => self.field = self.previous_field(),
            Key::Left => self.cycle(-1),
            Key::Right => self.cycle(1),
            Key::Backspace => {
                self.text_field().map(String::pop);
            }
            Key::Char(character) => match self.field {
                Field::Place if character == ' ' => self.cycle(1),
                _ => {
                    if let Some(text) = self.text_field() {
                        text.push(character);
                    }
                }
            },
        }
        FormOutcome::Editing
    }

    fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Project, Field::Agent, Field::Message, Field::Place];
        if self.place == Place::NewClone {
            fields.push(Field::Branch);
        }
        fields
    }

    fn next_field(&self) -> Field {
        let fields = self.fields();
        let at = fields
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0);
        fields[(at + 1) % fields.len()]
    }

    fn previous_field(&self) -> Field {
        let fields = self.fields();
        let at = fields
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0);
        fields[(at + fields.len() - 1) % fields.len()]
    }

    fn text_field(&mut self) -> Option<&mut String> {
        match self.field {
            Field::Project => Some(&mut self.project),
            Field::Message => Some(&mut self.message),
            Field::Branch => Some(&mut self.branch),
            Field::Agent | Field::Place => None,
        }
    }

    /// Left and Right step through the known projects, the agents, or the two places.
    fn cycle(&mut self, step: isize) {
        match self.field {
            Field::Project if !self.known_roots.is_empty() => {
                let at = self
                    .known_roots
                    .iter()
                    .position(|root| *root == self.project);
                let next = match at {
                    Some(at) => step_index(at, step, self.known_roots.len()),
                    None => 0,
                };
                self.project = self.known_roots[next].clone();
            }
            Field::Agent => {
                if let Some(agents) = self.agents.as_ref().filter(|agents| !agents.is_empty()) {
                    self.agent = step_index(self.agent, step, agents.len());
                }
            }
            Field::Place => {
                self.place = match self.place {
                    Place::InPlace => Place::NewClone,
                    Place::NewClone => Place::InPlace,
                };
            }
            _ => {}
        }
    }

    /// The `session-new` command, or why the form cannot send one yet.
    fn submit(&mut self) -> FormOutcome {
        if self.starting {
            return FormOutcome::Editing;
        }
        match self.command() {
            Ok(command) => {
                self.starting = true;
                self.problem = None;
                FormOutcome::Submit(command)
            }
            Err(problem) => {
                self.problem = Some(problem);
                FormOutcome::Editing
            }
        }
    }

    fn command(&self) -> Result<Command, String> {
        let dir = self.project.trim();
        if !dir.starts_with('/') {
            return Err("the project needs an absolute path".to_owned());
        }
        let agent = match &self.agents {
            None => return Err("the agents are still loading".to_owned()),
            Some(agents) => match agents.get(self.agent) {
                Some(agent) => agent.name.clone(),
                None => return Err("no agent is available: the daemon offers none".to_owned()),
            },
        };
        let clone = match self.place {
            Place::InPlace => None,
            Place::NewClone if self.branch.trim().is_empty() => {
                return Err("a new clone needs a branch".to_owned());
            }
            Place::NewClone => Some(self.branch.trim().to_owned()),
        };
        Ok(Command::SessionNew {
            dir: dir.to_owned(),
            agent: Some(agent),
            prompt: Some(self.message.clone()).filter(|message| !message.trim().is_empty()),
            clone,
        })
    }
}

fn step_index(at: usize, step: isize, len: usize) -> usize {
    let len = isize::try_from(len).expect("a short list");
    let at = isize::try_from(at).expect("a short list");
    usize::try_from((at + step).rem_euclid(len)).expect("rem_euclid is not negative")
}
