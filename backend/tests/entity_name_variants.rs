//! A person written with or without an honorific ("Dr Aida Ghalebeigi", "Dr. Aida Ghalebeigi",
//! "Aida Ghalebeigi") is one entity; different people with a shared first name, and non-people with
//! a title-like name, are not folded together.

mod common;

use common::TestApp;
use serde_json::{json, Value};

async fn write(app: &TestApp, name: &str, kind: &str, text: &str) -> Value {
    let out = app.tool("memory_write", json!({"subject_name": name, "subject_kind": kind, "text": text})).await;
    assert!(out.get("error").is_none(), "{out}");
    out
}

#[tokio::test]
async fn honorific_spellings_of_a_person_resolve_to_one_entity() {
    let app = TestApp::new().await;
    let first = write(&app, "Dr Aida Ghalebeigi", "person", "Aida Ghalebeigi lectures at Monash").await;
    let id = first["entity"]["id"].clone();

    for variant in ["Dr. Aida Ghalebeigi", "dr aida  ghalebeigi", "Aida Ghalebeigi"] {
        let again = write(&app, variant, "person", &format!("a fact written under {variant}")).await;
        assert_eq!(again["entity"]["id"], id, "{variant}");
    }
    let got = app.tool("entities_get", json!({"id": id})).await;
    let aliases: Vec<String> = got["aliases"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
    assert!(aliases.contains(&"Aida Ghalebeigi".to_string()), "{aliases:?}");
    assert_eq!(got["memory"].as_array().unwrap().len(), 4, "{got}");
}

#[tokio::test]
async fn the_plain_name_found_first_also_matches_a_later_titled_one() {
    let app = TestApp::new().await;
    let plain = write(&app, "Aida Ghalebeigi", "person", "plain").await;
    let titled = write(&app, "Dr. Aida Ghalebeigi", "person", "titled").await;
    assert_eq!(plain["entity"]["id"], titled["entity"]["id"]);
}

#[tokio::test]
async fn different_people_and_non_people_stay_apart() {
    let app = TestApp::new().await;
    let a = write(&app, "Sam Roberts", "person", "a").await;
    let b = write(&app, "Sam Leonard", "person", "b").await;
    assert_ne!(a["entity"]["id"], b["entity"]["id"]);

    // a single word never resolves to a titled person, and the other way round
    let titled = write(&app, "Dr Jane Smith", "person", "c").await;
    let single = write(&app, "Smith", "person", "d").await;
    assert_ne!(titled["entity"]["id"], single["entity"]["id"]);

    // honorific folding is for people only: the same spellings as organisations stay apart
    let org = write(&app, "Dr Jane Smith Ltd", "organisation", "e").await;
    let org2 = write(&app, "Jane Smith Ltd", "organisation", "f").await;
    assert_ne!(org["entity"]["id"], org2["entity"]["id"]);

    // one word after a title is not a full name, so these are two people
    let mr = write(&app, "Mr Sam", "person", "g").await;
    let sam = write(&app, "Sam", "person", "h").await;
    assert_ne!(mr["entity"]["id"], sam["entity"]["id"]);
    let who = write(&app, "Dr Who", "person", "i").await;
    let who2 = write(&app, "Who", "person", "j").await;
    assert_ne!(who["entity"]["id"], who2["entity"]["id"]);
}

#[tokio::test]
async fn a_titled_name_finds_a_person_stored_under_the_plain_name_as_an_alias() {
    let app = TestApp::new().await;
    // the entity's name is something else; the plain full name is only an alias (the alias lookup)
    let first = write(&app, "J. Roe", "person", "k").await;
    let id = first["entity"]["id"].clone();
    let upd = app.tool("entity_update", json!({"entity_id": id, "aliases": ["Jane Roe"]})).await;
    assert!(upd.get("error").is_none(), "{upd}");

    let titled = write(&app, "Dr   Jane  Roe", "person", "l").await;
    assert_eq!(titled["entity"]["id"], id, "{titled}");
    let got = app.tool("entities_get", json!({"id": id})).await;
    let aliases: Vec<String> = got["aliases"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
    assert!(aliases.contains(&"Dr Jane Roe".to_string()), "whitespace collapsed in the new alias: {aliases:?}");
}
