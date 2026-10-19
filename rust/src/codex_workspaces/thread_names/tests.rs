use super::*;
use tempfile::TempDir;

fn session<'a>(id: &'a str, cwd: Option<&'a str>) -> SessionRef<'a> {
    SessionRef { id, cwd }
}

fn write_state(dir: &Path, file: &str, rows: &[(&str, Option<&str>)]) {
    fs::create_dir_all(dir).unwrap();
    let conn = Connection::open(dir.join(file)).unwrap();
    conn.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT)")
        .unwrap();
    for (id, title) in rows {
        conn.execute(
            "INSERT INTO threads (id, title) VALUES (?1, ?2)",
            (id, title),
        )
        .unwrap();
    }
}

fn write_index(home: &Path, contents: &str) {
    fs::create_dir_all(home).unwrap();
    fs::write(home.join("session_index.jsonl"), contents).unwrap();
}

fn titles(home: &Path, env: Option<&str>, sessions: &[SessionRef<'_>]) -> HashMap<String, String> {
    thread_titles(home, env, sessions)
}

fn toml_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

#[test]
fn session_index_latest_line_wins_and_blank_names_are_ignored() {
    let tmp = TempDir::new().unwrap();
    write_index(
        tmp.path(),
        concat!(
            "{\"id\":\"a\",\"thread_name\":\"First\"}\n",
            "{\"id\":\"a\",\"thread_name\":\"  Renamed  \"}\n",
            "{\"id\":\"b\",\"thread_name\":\"   \"}\n",
            "{\"id\":\"c\",\"thread_name\":\"Not requested\"}\n",
            "not json\n",
            "{\"id\":\"d\",\"thread_name\":\"No trailing newline\"}",
        ),
    );
    let found = titles(
        tmp.path(),
        None,
        &[session("a", None), session("b", None), session("d", None)],
    );
    assert_eq!(found.get("a").map(String::as_str), Some("Renamed"));
    assert_eq!(
        found.get("d").map(String::as_str),
        Some("No trailing newline")
    );
    assert!(!found.contains_key("b"));
    assert!(!found.contains_key("c"));
}

#[test]
fn session_index_skips_lines_over_64_kib() {
    let tmp = TempDir::new().unwrap();
    let huge = "x".repeat(70 * 1024);
    write_index(
        tmp.path(),
        &format!(
            "{{\"id\":\"a\",\"thread_name\":\"{huge}\"}}\n{{\"id\":\"b\",\"thread_name\":\"After\"}}\n"
        ),
    );
    let found = titles(tmp.path(), None, &[session("a", None), session("b", None)]);
    assert!(!found.contains_key("a"));
    assert_eq!(found.get("b").map(String::as_str), Some("After"));
}

#[test]
fn database_title_is_the_fallback_and_the_index_wins() {
    let tmp = TempDir::new().unwrap();
    write_state(
        tmp.path(),
        "state_5.sqlite",
        &[
            ("a", Some("Database name")),
            ("b", Some("  Trimmed  ")),
            ("c", None),
            ("d", Some("   ")),
        ],
    );
    write_index(
        tmp.path(),
        "{\"id\":\"a\",\"thread_name\":\"Index name\"}\n",
    );
    let found = titles(
        tmp.path(),
        None,
        &[
            session("a", None),
            session("b", None),
            session("c", None),
            session("d", None),
            session("missing", None),
        ],
    );
    assert_eq!(found.get("a").map(String::as_str), Some("Index name"));
    assert_eq!(found.get("b").map(String::as_str), Some("Trimmed"));
    assert_eq!(found.len(), 2);
}

#[test]
fn database_titles_are_bounded_but_index_names_are_not() {
    // Multi-byte characters check that the bound counts characters, not bytes.
    let prompt = "é🙂".repeat(50_000);
    let explicit_name = "n".repeat(300);
    for schema in [
        "CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, agent_path TEXT)",
        "CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT)",
    ] {
        let tmp = TempDir::new().unwrap();
        let conn = Connection::open(tmp.path().join("state_5.sqlite")).unwrap();
        conn.execute_batch(schema).unwrap();
        for (id, title) in [
            ("prompt", prompt.as_str()),
            ("padded", "  Short title \n"),
            ("named", prompt.as_str()),
        ] {
            conn.execute(
                "INSERT INTO threads (id, title) VALUES (?1, ?2)",
                (id, title),
            )
            .unwrap();
        }
        drop(conn);
        write_index(
            tmp.path(),
            &format!(
                "{{\"id\":\"named\",\"thread_name\":\"{explicit_name}\",\"updated_at\":\"now\"}}\n"
            ),
        );

        let found = titles(
            tmp.path(),
            None,
            &[
                session("prompt", None),
                session("padded", None),
                session("named", None),
            ],
        );
        let prompt_title = found.get("prompt").expect("prompt title");
        assert_eq!(prompt_title.chars().count(), 256, "{schema}");
        assert_eq!(prompt_title, &"é🙂".repeat(128), "{schema}");
        assert_eq!(found.get("padded").map(String::as_str), Some("Short title"));
        assert_eq!(found.get("named"), Some(&explicit_name));
    }
}

#[test]
fn newest_state_database_version_is_used() {
    let tmp = TempDir::new().unwrap();
    write_state(tmp.path(), "state_5.sqlite", &[("a", Some("old"))]);
    write_state(tmp.path(), "state_10.sqlite", &[("a", Some("newest"))]);
    write_state(tmp.path(), "state_7.sqlite", &[("a", Some("middle"))]);
    let found = titles(tmp.path(), None, &[session("a", None)]);
    assert_eq!(found.get("a").map(String::as_str), Some("newest"));
}

#[test]
fn missing_or_corrupt_database_yields_no_titles() {
    let tmp = TempDir::new().unwrap();
    assert!(titles(tmp.path(), None, &[session("a", None)]).is_empty());
    fs::write(tmp.path().join("state_5.sqlite"), b"not a database").unwrap();
    assert!(titles(tmp.path(), None, &[session("a", None)]).is_empty());
}

#[test]
fn config_sqlite_home_must_be_absolute_and_top_level() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let other = tmp.path().join("other");
    fs::create_dir_all(&home).unwrap();
    write_state(&home, "state_5.sqlite", &[("a", Some("from home"))]);
    write_state(&other, "state_5.sqlite", &[("a", Some("from config"))]);

    let config = home.join("config.toml");
    fs::write(
        &config,
        format!("sqlite_home = \"{}\"\n", toml_path(&other)),
    )
    .unwrap();
    let found = titles(&home, None, &[session("a", None)]);
    assert_eq!(found.get("a").map(String::as_str), Some("from config"));

    fs::write(&config, "sqlite_home = \"relative/dir\"\n").unwrap();
    let found = titles(&home, None, &[session("a", None)]);
    assert_eq!(found.get("a").map(String::as_str), Some("from home"));

    fs::write(
        &config,
        format!(
            "model = \"x\"\n[projects.p]\nsqlite_home = \"{}\"\n",
            toml_path(&other)
        ),
    )
    .unwrap();
    let found = titles(&home, None, &[session("a", None)]);
    assert_eq!(found.get("a").map(String::as_str), Some("from home"));
}

#[test]
fn config_top_level_key_survives_a_malformed_table_section() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let other = tmp.path().join("other");
    fs::create_dir_all(&home).unwrap();
    write_state(&other, "state_5.sqlite", &[("a", Some("from config"))]);
    fs::write(
        home.join("config.toml"),
        format!(
            "sqlite_home = \"{}\"\r\n[broken\r\nkey = \r\n",
            toml_path(&other)
        ),
    )
    .unwrap();
    let found = titles(&home, None, &[session("a", None)]);
    assert_eq!(found.get("a").map(String::as_str), Some("from config"));
}

#[test]
fn config_beats_environment_which_beats_codex_home() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let from_config = tmp.path().join("cfg");
    let from_env = tmp.path().join("env");
    fs::create_dir_all(&home).unwrap();
    write_state(&home, "state_5.sqlite", &[("a", Some("home"))]);
    write_state(&from_env, "state_5.sqlite", &[("a", Some("env"))]);
    write_state(&from_config, "state_5.sqlite", &[("a", Some("config"))]);
    let env = from_env.to_string_lossy().into_owned();

    let found = titles(&home, Some(&env), &[session("a", None)]);
    assert_eq!(found.get("a").map(String::as_str), Some("env"));

    fs::write(
        home.join("config.toml"),
        format!("sqlite_home = '{}'\n", from_config.display()),
    )
    .unwrap();
    let found = titles(&home, Some(&env), &[session("a", None)]);
    assert_eq!(found.get("a").map(String::as_str), Some("config"));
}

#[test]
fn relative_environment_home_resolves_against_the_session_cwd() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let cwd_one = tmp.path().join("repo-one");
    let cwd_two = tmp.path().join("repo-two");
    fs::create_dir_all(&home).unwrap();
    write_state(&home, "state_5.sqlite", &[("c", Some("home title"))]);
    write_state(&cwd_one.join("db"), "state_5.sqlite", &[("a", Some("one"))]);
    write_state(&cwd_two.join("db"), "state_5.sqlite", &[("b", Some("two"))]);
    let one = cwd_one.to_string_lossy().into_owned();
    let two = cwd_two.to_string_lossy().into_owned();

    let found = titles(
        &home,
        Some("db"),
        &[
            session("a", Some(&one)),
            session("b", Some(&two)),
            // No working directory: a relative path cannot be resolved, use the home.
            session("c", None),
        ],
    );
    assert_eq!(found.get("a").map(String::as_str), Some("one"));
    assert_eq!(found.get("b").map(String::as_str), Some("two"));
    assert_eq!(found.get("c").map(String::as_str), Some("home title"));
}

#[test]
fn no_sessions_reads_nothing() {
    let tmp = TempDir::new().unwrap();
    write_index(tmp.path(), "{\"id\":\"a\",\"thread_name\":\"x\"}\n");
    assert!(titles(tmp.path(), None, &[]).is_empty());
}
