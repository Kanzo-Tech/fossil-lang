//! **`run` refuses a cloud destination, and it refuses it before it reads a
//! credential.**
//!
//! That order is the whole content of this file, and it is what made a field in
//! the `--creds-stdin` wire dead without anyone noticing. The payload carried a
//! `dest` section holding a provider-typed secret for the destination;
//! `local_dest_dir` returns `None` for any URL with a scheme other than
//! `file://`, `run` turns that into an error, and the section was never
//! consulted on any path. A host could fill it in correctly and be ignored,
//! which reads as support for something that does not exist.
//!
//! The section is gone. What has to stay true for that deletion to be right is
//! the refusal itself, so it is asserted here rather than described in
//! `creds.rs`, whose prose would otherwise be the only thing holding it.
//!
//! **What this cannot prove:** that a cloud destination *should* be refused.
//! The `DataFusion` write path is not wired to an object store, and the day it is
//! this file goes red — which is the correct way for it to fail. It is a
//! statement about today, not an argument for tomorrow.

#![cfg(not(target_arch = "wasm32"))]
// `{User.id}` is fossil's interpolation hole, not a Rust format argument.
#![allow(clippy::literal_string_with_formatting_args)]

use std::path::Path;

const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")
User := io.csv(\"users.csv\")
People : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name     = User.name
";

const DOCUMENT: &str = "\
PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:Person {
  ex:name xsd:string
}
";

fn fixture(dir: &Path) {
    std::fs::write(dir.join("prog.fossil"), PROGRAM).expect("write program");
    std::fs::write(dir.join("person.shex"), DOCUMENT).expect("write document");
    std::fs::write(dir.join("users.csv"), "id,name\n1,Alice\n").expect("write csv");
}

/// Every cloud scheme the resolver knows how to render a secret for is refused,
/// and the message names the destination so the operator can see which of their
/// arguments is the problem.
#[test]
fn a_cloud_destination_is_refused_and_says_which() {
    for dest in ["s3://bucket/prefix", "az://container/prefix", "gcs://b/p"] {
        let dir = tempfile::tempdir().expect("tempdir");
        fixture(dir.path());
        let err = fossil_engine::run(
            &dir.path().join("prog.fossil"),
            dest,
            &fossil_engine::RunCreds::default(),
            None,
        )
        .expect_err("the write path is a local directory; a cloud dest must be refused");
        let message = format!("{err}");
        assert!(
            message.contains(dest),
            "the refusal must name the destination it refused; got `{message}`"
        );
    }
}

/// And the refusal comes from the DESTINATION, not from a missing credential:
/// the same URL is refused identically whether or not the payload carries
/// secrets for it. This is the assertion that makes the deleted `dest` section
/// dead rather than merely unused — nothing a host can put in the payload
/// changes the answer.
#[test]
fn the_refusal_does_not_depend_on_what_the_creds_payload_carries() {
    let dir = tempfile::tempdir().expect("tempdir");
    fixture(dir.path());
    let dest = "s3://bucket/prefix";

    let bare = fossil_engine::run(
        &dir.path().join("prog.fossil"),
        dest,
        &fossil_engine::RunCreds::default(),
        None,
    )
    .expect_err("refused");

    // A payload naming a connection WITH a secret, plus a `dest` section of the
    // shape the wire used to carry. Both are accepted by the parser and neither
    // reaches the destination decision.
    let loaded = fossil_engine::RunCreds::from_json(
        r#"{ "dest": { "secret": { "type": "s3", "params": { "KEY_ID": "AKIA" } } },
             "connections": { "sales": { "url": "s3://bucket/prefix",
                                         "secret": { "type": "s3",
                                                     "params": { "KEY_ID": "AKIA" } } } } }"#,
    )
    .expect("the payload parses");
    let with_creds = fossil_engine::run(&dir.path().join("prog.fossil"), dest, &loaded, None)
        .expect_err("still refused");

    assert_eq!(
        format!("{bare}"),
        format!("{with_creds}"),
        "credentials must not change whether a destination is reachable"
    );
}

/// The two spellings of a local destination both work, and produce the same
/// tree — the control for the tests above, which would otherwise pass against a
/// `run` that refused everything.
#[test]
fn both_spellings_of_a_local_destination_are_accepted() {
    for dest in ["plain", "file://"] {
        let dir = tempfile::tempdir().expect("tempdir");
        fixture(dir.path());
        let out = dir.path().join("out");
        let url = if dest == "file://" {
            format!("file://{}", out.display())
        } else {
            out.to_string_lossy().into_owned()
        };
        let status = fossil_engine::run(
            &dir.path().join("prog.fossil"),
            &url,
            &fossil_engine::RunCreds::default(),
            None,
        )
        .unwrap_or_else(|e| panic!("a local dest spelt `{dest}` must run: {e}"));
        assert_eq!(status.vertices.len(), 1);
        assert!(out.join("vertex/Person/").is_dir());
    }
}
