pub use cockpit_core::approval::*;

mod terminal;
pub use terminal::{
    choose, choose_or_remote, edit, explanation, reserve_last_row, run, terminal_size,
};
