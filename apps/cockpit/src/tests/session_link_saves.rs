use super::*;
use crate::teams::{self, HostAction, LinkProposal, TaskKey, VerifiedSession, links, store};

#[test]
fn chief_completion_closes_links_and_refuses_projection_when_invalidation_fails() {
    for fail_closure in [false, true] {
        let fixture = Fixture::new();
        let project = fixture.root.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        let mut todos = TodoList::load();
        let id = todos.add("project", project.to_str().unwrap(), "Joint work");
        let connection = store::open(&crate::paths::team_database()).unwrap();
        link_fixture::approve(&connection, &format!("path:{}", project.display()), &id);
        if fail_closure {
            connection.execute_batch("CREATE TRIGGER refuse_projection BEFORE UPDATE ON link_work BEGIN SELECT RAISE(ABORT,'closure refused'); END;").unwrap();
        }
        let result = todos.project_chief(&[json!({"task_id":id,"assignment":"chief-work","worker":SESSION_A,"status":"done"})]);
        if fail_closure {
            assert!(result.is_err());
            assert_eq!(TodoList::load().status_of(&id), "todo");
            assert_eq!(link_fixture::state(&connection, &id), "active");
        } else {
            result.unwrap();
            assert_eq!(TodoList::load().status_of(&id), "done");
            assert_eq!(link_fixture::state(&connection, &id), "closed");
            assert_eq!(connection.query_row("SELECT state FROM messages WHERE id=?1", [format!("{id}-queued")], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
        }
    }
}

#[test]
fn todo_first_snapshot_save_excludes_initial_link_enrollment_until_json_is_durable() {
    let fixture = Fixture::new();
    let project = fixture.root.join("project");
    fs::create_dir_all(project.join(".git")).unwrap();
    assert!(!crate::paths::team_database().exists());
    let (reached, before_json) = std::sync::mpsc::channel();
    let (resume, released) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        TODO_SAVE_BARRIER.with(|barrier| *barrier.borrow_mut() = Some((reached, released)));
        TodoList::load().add("project", project.to_str().unwrap(), "First work")
    });
    before_json.recv_timeout(Duration::from_secs(5)).unwrap();
    let enrollment = store::open(&crate::paths::team_database());
    resume.send(()).unwrap();
    let id = writer.join().unwrap();
    assert!(enrollment.is_err(), "initial enrollment must wait even when no collaboration store existed before the save");
    assert_eq!(TodoList::load().items[0].id(), id);
    assert!(store::open(&crate::paths::team_database()).is_ok());
}

#[test]
fn todo_stale_editor_save_preserves_newer_shared_work_and_its_draft() {
    for omission in [false, true] {
        let fixture = Fixture::new();
        let project = fixture.root.join("project");
        let peer = fixture.root.join("peer");
        for path in [&project, &peer] { fs::create_dir_all(path.join(".git")).unwrap(); }
        let mut current = TodoList::load();
        let other = current.add("project", project.to_str().unwrap(), "Other work");
        let mut stale = TodoList::load();
        let id = current.add("project", project.to_str().unwrap(), "Joint work");
        if !omission {
            assert!(current.set_status(&id, "done", ""));
            stale = TodoList::load();
            assert!(current.set_status(&id, "todo", ""));
        }
        let edited = if omission { &other } else { &id };
        let item = stale.items.iter().find(|item| item.id() == *edited).unwrap();
        let mut draft = crate::todo_editor::Draft::create("project", project.to_str().unwrap(), Some(item)).unwrap();
        draft.checkpoint("Edited title", "Recoverable notes", Some(crate::todo::Category::Task)).unwrap();
        let task = TaskKey { project: format!("path:{}", project.display()), task_id: id.clone() };
        let live = vec![
            VerifiedSession { session_id:SESSION_A.into(), agent:"claude".into(), project:task.project.clone(), runtime:"first-runtime".into() },
            VerifiedSession { session_id:SESSION_B.into(), agent:"claude".into(), project:format!("path:{}", peer.display()), runtime:"peer-runtime".into() },
        ];
        let mut connection = store::open(&crate::paths::team_database()).unwrap();
        let proposal = LinkProposal { submission_key:"new-approval".into(), task, peer_session_id:SESSION_B.into(), purpose:"Joint work".into() };
        let receipt = links::propose(&mut connection, &live[0], &proposal, &live, 0).unwrap().result.unwrap();
        let hash = connection.query_row("SELECT mapping_hash FROM link_proposals WHERE id=?1", [receipt["proposal_id"].as_str().unwrap()], |row| row.get(0)).unwrap();
        let approved = store::control(&mut connection, &HostAction::LinkApprove {
            request_id:"approve-new".into(), proposal_id:receipt["proposal_id"].as_str().unwrap().into(), expected_revision:1, mapping_hash:hash,
        }, &live, 1).unwrap().result.unwrap();
        let link = approved["link_id"].as_str().unwrap();
        let message = teams::LinkSubmission { link_id:link.into(), submission_key:"safe-message".into(), kind:teams::MessageKind::Notice, body:"Coordination".into(), thread_id:None };
        links::accept(&mut connection, &live[0], &message, &live, 1000).unwrap();
        let before = fs::read(crate::paths::todo()).unwrap();
        let previous = stale.items.clone();
        let error = draft.finish(&mut stale).unwrap_err();
        assert!(error.to_string().contains("stale TODO snapshot"), "{error}");
        assert_eq!(fs::read(crate::paths::todo()).unwrap(), before);
        assert_eq!(stale.items, previous, "failed editor save must restore its snapshot");
        let draft_path = crate::paths::state_dir().join("todo-drafts").join(format!("{}.json", draft.token));
        assert!(draft_path.is_file(), "rejected editor content must stay recoverable");
        assert!(current.set_status(&id, "done", ""));
        assert!(current.set_status(&id, "todo", ""));
        assert_eq!(connection.query_row("SELECT state FROM links WHERE id=?1", [link], |row| row.get::<_, String>(0)).unwrap(), "closed");
        assert_eq!(connection.query_row("SELECT state FROM messages", [], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
        assert!(links::accept(&mut connection, &live[0], &message, &live, 2000).is_err());
        assert!(store::claim(&mut connection, &live[1], "after-reopen", &live, 2001).unwrap().is_none());
    }
}
