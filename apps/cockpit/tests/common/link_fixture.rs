use super::LinkConnection as Connection;

pub fn approve(connection: &Connection, project: &str, id: &str) {
    let task_key = serde_json::json!({"project": project, "task_id": id}).to_string();
    connection.execute("INSERT INTO link_work VALUES (?1,?2,?3,4,'open',10)", (&task_key, project, id)).unwrap();
    for (suffix, status) in [("approved", "approved"), ("pending", "pending")] {
        let proposal = format!("{id}-{suffix}");
        connection.execute("INSERT INTO link_proposals (id,sender,submission_key,fingerprint,task_key,purpose,revision,mapping_hash,work_generation,status,created_at)
            VALUES (?1,'a',?1,'fingerprint',?2,'Joint work',1,'hash',4,?3,10)", (&proposal, &task_key, status)).unwrap();
        for (slot, session, owner) in [(0, "a", project), (1, "b", "path:/other")] {
            connection.execute("INSERT INTO link_participants VALUES (?1,?2,?3,'claude',?4,'runtime')", (&proposal, slot, session, owner)).unwrap();
        }
    }
    connection.execute("INSERT INTO links VALUES (?1,?2,1,'hash',?3,4,'a','b','epoch','active',NULL,20,NULL)",
        (id, format!("{id}-approved"), &task_key)).unwrap();
    for (label, state, attempts) in [("queued", "queued", 0), ("retry", "queued", 1), ("ready", "ready", 0),
        ("submitting", "submitting", 1), ("uncertain", "uncertain", 1), ("received", "received", 1)] {
        connection.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime,scope_kind,link_id,attempts)
            VALUES (?1,?2,'a','b','epoch','epoch',?1,'fingerprint','{}',?3,0,0,'runtime','link',?4,?5)",
            (format!("{id}-{label}"), &task_key, state, id, attempts)).unwrap();
    }
    connection.execute("INSERT INTO attempts VALUES (?1,?2,1,'not_submitted',NULL,10,5)",
        (format!("{id}-attempt"), format!("{id}-retry"))).unwrap();
}

pub fn state(connection: &Connection, id: &str) -> String {
    connection.query_row("SELECT state FROM links WHERE id=?1", [id], |row| row.get(0)).unwrap()
}
