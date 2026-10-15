//! The roles a queue plays, as an editor reads them
//! (`frontend::highlight::queue_roles`, #645 B5, B8): the table `queue::ROLES`
//! is the one list, and the table of `docs/language.md` a copy, held equal
//! here both ways: each role's entry, its verb, the parameters between its
//! parentheses and its `from Q`, or none.

use std::path::Path;

use serq::frontend::highlight::queue_roles;

/// The rows of the roles table: the role and its entries column.
fn table() -> Vec<(String, String)> {
    let doc =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/language.md"))
            .unwrap();
    let at = doc.find("| Role | Entries | The queue |").unwrap();
    doc[at..]
        .lines()
        .skip(2)
        .take_while(|l| l.starts_with("| `"))
        .map(|l| {
            let cells: Vec<&str> = l.split(" | ").collect();
            let role = cells[0].trim_start_matches("| ").trim_matches('`');
            (role.to_string(), cells[1].to_string())
        })
        .collect()
}

#[test]
fn every_role_is_its_row_of_the_language() {
    let rows = table();
    let names: Vec<&str> = rows.iter().map(|(r, _)| r.as_str()).collect();
    let roles = queue_roles();
    assert_eq!(names, roles.iter().map(|r| r.name).collect::<Vec<_>>());
    for (r, (_, entries)) in roles.iter().zip(&rows) {
        let Some(verb) = r.verb else {
            assert_eq!(entries, "none", "`{}`", r.name);
            continue;
        };
        // `decode (prompt) from Q`, `route { … }`, `transfer (n)`, or none
        let first = entries
            .split('`')
            .nth(1)
            .unwrap_or_else(|| panic!("`{}` has an entry, and its row says {entries}", r.name));
        // the verb, the whole first word: `pre` is no `prefill`
        assert_eq!(
            first.split([' ', '{']).next(),
            Some(verb),
            "`{}`: {entries}",
            r.name
        );
        assert_eq!(
            entries.ends_with(", or none"),
            r.entry_optional,
            "`{}`: {entries}",
            r.name
        );
        assert_eq!(first.contains(" from Q"), r.from, "`{}`: {entries}", r.name);
        let params = first.split_once('(').map_or(0, |(_, rest)| {
            rest[..rest.find(')').unwrap()].split(", ").count()
        });
        assert_eq!(params, r.params, "`{}`: {entries}", r.name);
    }
}
