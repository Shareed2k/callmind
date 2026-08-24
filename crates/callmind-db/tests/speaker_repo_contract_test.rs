//! Speaker embeddings, stored so a voice can be recognised in a later call.
//!
//! Runs against every backend: the vector is the only new column shape here, and
//! it is stored as bytes rather than a native array so SQLite and Postgres take
//! the same code.

mod backend;

use callmind_core::{Call, CallDirection, OrgId, SpeakerId};
use callmind_db::sql::SqlCallRepository;
use callmind_db::{CallRepository, SpeakerRepository};

async fn seeded_call(repo: &SqlCallRepository, ext: &str) -> callmind_core::CallId {
    let call = Call::new(
        OrgId::DEFAULT,
        Some(ext.to_string()),
        CallDirection::Incoming,
        None,
        None,
        None,
    );
    repo.create(&call).await.expect("create call");
    call.id
}

#[tokio::test]
async fn embeddings_round_trip_on_every_backend() {
    for (name, conn) in backend::all("t_speakers").await {
        let repo = SqlCallRepository::new(conn);
        let call = seeded_call(&repo, "ext-spk").await;

        assert!(
            repo.list_named_speakers(OrgId::DEFAULT)
                .await
                .expect("list")
                .is_empty(),
            "{name}: nothing named yet"
        );

        let dad = vec![0.9f32, 0.1, 0.0, -0.4];
        repo.save_speaker_embedding(call, SpeakerId::new(0), &dad)
            .await
            .expect("save");
        repo.save_speaker_embedding(call, SpeakerId::new(1), &[0.0, 1.0, 0.0, 0.2])
            .await
            .expect("save");

        // Saving the same speaker again replaces rather than duplicates: a call
        // is reprocessed often and each speaker has one embedding.
        repo.save_speaker_embedding(call, SpeakerId::new(0), &dad)
            .await
            .expect("save again");

        let stored = repo.speakers_for_call(call).await.expect("read back");
        assert_eq!(stored.len(), 2, "{name}: one row per speaker");
        let first = stored
            .iter()
            .find(|s| s.speaker_id == SpeakerId::new(0))
            .expect("speaker 0");
        assert_eq!(
            first.embedding, dad,
            "{name}: the vector survives the round trip exactly"
        );
        assert!(first.name.is_none(), "{name}: not named yet");

        // Naming one speaker is what turns it into something recognisable later.
        repo.name_speaker(call, SpeakerId::new(0), "Папа")
            .await
            .expect("name");
        let known = repo
            .list_named_speakers(OrgId::DEFAULT)
            .await
            .expect("list");
        assert_eq!(known.len(), 1, "{name}: only the named one is an exemplar");
        assert_eq!(known[0].0, "Папа", "{name}");
        assert_eq!(known[0].1, dad, "{name}: with its vector");

        // Renaming replaces the name rather than adding a second exemplar.
        repo.name_speaker(call, SpeakerId::new(0), "Отец")
            .await
            .expect("rename");
        let known = repo
            .list_named_speakers(OrgId::DEFAULT)
            .await
            .expect("list");
        assert_eq!(known.len(), 1, "{name}");
        assert_eq!(known[0].0, "Отец", "{name}");
    }
}

/// Deleting a call must take its embeddings with it -- they are personal data
/// and a dangling voice print is worse than none.
#[tokio::test]
async fn deleting_a_call_removes_its_embeddings() {
    for (name, conn) in backend::all("t_speakers_delete").await {
        let repo = SqlCallRepository::new(conn);
        let call = seeded_call(&repo, "ext-del").await;
        repo.save_speaker_embedding(call, SpeakerId::new(0), &[1.0, 0.0])
            .await
            .expect("save");
        repo.name_speaker(call, SpeakerId::new(0), "Мама")
            .await
            .expect("name");

        assert!(repo.delete(call).await.expect("delete"), "{name}");
        assert!(
            repo.speakers_for_call(call).await.expect("read").is_empty(),
            "{name}: embeddings went with the call"
        );
        assert!(
            repo.list_named_speakers(OrgId::DEFAULT)
                .await
                .expect("list")
                .is_empty(),
            "{name}: and so did the exemplar"
        );
    }
}
