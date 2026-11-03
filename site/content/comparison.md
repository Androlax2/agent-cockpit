# Where Torlek fits

Torlek adds a personal orchestration layer around supported coding-agent CLIs.
It keeps sessions, work, questions and host approvals together through a CLI and terminal UI.

| Starting point | What Torlek adds |
| --- | --- |
| Running one provider CLI | A common session list and lifecycle across supported providers, subject to each adapter's capability evidence |
| Managing terminal panes manually | tmux-backed sessions with recorded state, project registration and a first-run readiness checklist |
| Giving an agent host commands directly | A separate owner approval path for outside-sandbox requests |

Use the [provider capability evidence](../../docs/USER-GUIDE.md#providers-and-capability-evidence) and [current install status](../../docs/USER-GUIDE.md#install-status) when deciding whether the Alpha fits your work.
Torlek is a personal installation with one human owner; shared multi-human control and native Windows execution are outside its current scope.
See the [scope decision](../../docs/design/greenfield/01-scope-and-principles.md).
