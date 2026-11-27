use super::*;

#[test]
fn watch_cache_reuses_modules_with_stable_ids() {
  let dir = tempfile::tempdir().unwrap();
  fs::write(
    dir.path().join("value.ts"),
    "export const value: number = 1;",
  )
  .unwrap();
  fs::write(
    dir.path().join("other.ts"),
    "export const OTHER_MARKER = 2;",
  )
  .unwrap();
  fs::write(dir.path().join("dep.js"), "export default 'js';").unwrap();
  let entry = dir.path().join("app.ts");
  fs::write(&entry, "import {value} from './value.ts'; import {OTHER_MARKER} from './other.ts'; import dep from './dep'; globalThis.result = [value, OTHER_MARKER, dep].join(':');").unwrap();
  let cache = Rc::new(Cache::default());
  let run = |files: &BBTreeMap<String, String>| {
    let engine = apps_runtime::host::context::JsContext::new(
      apps_runtime::jsc::JavaScriptCore::load().unwrap(),
      Rc::default(),
      apps_runtime::jsc::Role::Main,
    )
    .unwrap();
    engine.load_bundle(files, "app.js").unwrap();
    engine.eval("result", "test").unwrap()
  };
  let line = |files: &BBTreeMap<String, String>| {
    files["app.js"]
      .lines()
      .find(|line| line.contains("OTHER_MARKER = 2"))
      .unwrap()
      .to_owned()
  };
  let first = compile(
    &entry,
    Realm::Main,
    None,
    Rc::default(),
    false,
    Some(cache.clone()),
  )
  .unwrap();
  assert_eq!(run(&first), "1:2:js");
  fs::write(
    dir.path().join("value.ts"),
    "export const value: number = 30;",
  )
  .unwrap();
  let watched = Rc::default();
  let second = compile(
    &entry,
    Realm::Main,
    None,
    Rc::clone(&watched),
    false,
    Some(cache.clone()),
  )
  .unwrap();
  assert_eq!(run(&second), "30:2:js");
  assert_eq!(line(&first), line(&second));
  assert!(
    watched
      .borrow()
      .contains(&dir.path().join("other.ts").canonicalize().unwrap())
  );
  fs::write(&entry, "import {OTHER_MARKER} from './other.ts'; import dep from './dep'; import {value} from './value.ts'; globalThis.result = [value, OTHER_MARKER, dep].join(':');").unwrap();
  fs::write(dir.path().join("dep.ts"), "export default 'ts';").unwrap();
  cache.resolutions.borrow_mut().clear();
  let third = compile(
    &entry,
    Realm::Main,
    None,
    Rc::default(),
    false,
    Some(cache.clone()),
  )
  .unwrap();
  assert_eq!(run(&third), "30:2:ts");
  assert_eq!(line(&first), line(&third));
}

#[test]
fn watch_cache_keeps_css_imports_and_assets() {
  let dir = tempfile::tempdir().unwrap();
  let entry = dir.path().join("page.ts");
  fs::write(&entry, "import './a.css';").unwrap();
  fs::write(
    dir.path().join("a.css"),
    "@import './theme.css'; .a{background:url(./bg.png)}",
  )
  .unwrap();
  fs::write(dir.path().join("bg.png"), b"png").unwrap();
  let cache = Rc::new(Cache::default());
  for color in ["red", "blue"] {
    fs::write(dir.path().join("theme.css"), format!(".t{{color:{color}}}")).unwrap();
    let watched = Rc::new(RefCell::new(BTreeSet::new()));
    let files = compile(
      &entry,
      Realm::Renderer,
      None,
      watched.clone(),
      false,
      Some(cache.clone()),
    )
    .unwrap();
    assert!(files["app.js"].contains(color), "{color}");
    let root = dir.path().canonicalize().unwrap();
    for name in ["theme.css", "bg.png"] {
      assert!(
        watched.borrow().contains(&root.join(name)),
        "{name} {:?}",
        watched.borrow()
      );
    }
  }
}

#[test]
fn watch_cache_follows_package_env() {
  let dir = tempfile::tempdir().unwrap();
  let entry = dir.path().join("app.ts");
  fs::write(&entry, "globalThis.x = import.meta.env.X;").unwrap();
  let cache = Rc::new(Cache::default());
  for value in ["first-env", "second-env"] {
    fs::write(
      dir.path().join("package.json"),
      json!({"env": {"X": value}}).to_string(),
    )
    .unwrap();
    cache.resolutions.borrow_mut().clear();
    let files = compile(
      &entry,
      Realm::Main,
      None,
      Rc::default(),
      false,
      Some(cache.clone()),
    )
    .unwrap();
    assert!(files["app.js"].contains(value), "{value}");
  }
}

#[test]
fn failed_builds_still_watch_package_json() {
  let dir = tempfile::tempdir().unwrap();
  let entry = dir.path().join("app.ts");
  fs::write(&entry, "").unwrap();
  fs::write(dir.path().join("package.json"), "{").unwrap();
  let watched = Rc::new(RefCell::new(BTreeSet::new()));
  assert!(compile(&entry, Realm::Main, None, watched.clone(), false, None).is_err());
  let root = dir.path().canonicalize().unwrap();
  assert!(watched.borrow().contains(&root.join("package.json")));
  assert!(watched.borrow().contains(&entry));
}

#[test]
fn watch_cache_reparses_modules_whose_optional_dependency_is_gone() {
  let dir = tempfile::tempdir().unwrap();
  for (path, source) in [
    (
      "node_modules/wrap/index.js",
      "let value; try { value = require('opt'); } catch { value = 'none'; } module.exports = value;",
    ),
    ("node_modules/wrap/package.json", "{}"),
    ("node_modules/opt/package.json", "{}"),
    ("node_modules/opt/index.js", "module.exports = 'opt';"),
    (
      "app.js",
      "import value from 'wrap'; globalThis.result = value;",
    ),
  ] {
    fs::create_dir_all(dir.path().join(path).parent().unwrap()).unwrap();
    fs::write(dir.path().join(path), source).unwrap();
  }
  let entry = dir.path().join("app.js");
  let cache = Rc::new(Cache::default());
  let run = || {
    let files = compile(
      &entry,
      Realm::Main,
      None,
      Rc::default(),
      false,
      Some(cache.clone()),
    )
    .unwrap();
    let engine = apps_runtime::host::context::JsContext::new(
      apps_runtime::jsc::JavaScriptCore::load().unwrap(),
      Rc::default(),
      apps_runtime::jsc::Role::Main,
    )
    .unwrap();
    engine.load_bundle(&files, "app.js").unwrap();
    engine.eval("result", "test").unwrap()
  };
  assert_eq!(run(), "opt");
  fs::remove_dir_all(dir.path().join("node_modules/opt")).unwrap();
  cache.resolutions.borrow_mut().clear();
  assert_eq!(run(), "none");
}

#[test]
fn watch_includes_the_tsconfig_chain_without_paths() {
  let dir = tempfile::tempdir().unwrap();
  for (path, source) in [
    (
      "tsconfig.base.json",
      r#"{"compilerOptions":{"strict":true}}"#,
    ),
    ("tsconfig.json", r#"{"extends":"./tsconfig.base"}"#),
    ("src/a.ts", "export const dep = 'A_MARKER';"),
    (
      "node_modules/dep/package.json",
      r#"{"type":"module","main":"index.js"}"#,
    ),
    ("node_modules/dep/index.js", "export const dep = 'dep';"),
    (
      "app.ts",
      "import { dep } from 'dep'; globalThis.result = dep;",
    ),
  ] {
    fs::create_dir_all(dir.path().join(path).parent().unwrap()).unwrap();
    fs::write(dir.path().join(path), source).unwrap();
  }
  let root = dir.path().canonicalize().unwrap();
  let entry = dir.path().join("app.ts");
  let cache = Rc::new(Cache::default());
  let build = || {
    let watched = Rc::<RefCell<BTreeSet<PathBuf>>>::default();
    let files = compile(
      &entry,
      Realm::Main,
      None,
      watched.clone(),
      false,
      Some(cache.clone()),
    )
    .unwrap();
    for name in ["tsconfig.json", "tsconfig.base.json"] {
      assert!(
        watched.borrow().contains(&root.join(name)),
        "{name} {:?}",
        watched.borrow()
      );
    }
    files
  };
  build();
  // Cached resolutions skip the tsconfig lookup.
  build();
  fs::write(
    dir.path().join("tsconfig.base.json"),
    r#"{"compilerOptions":{"paths":{"dep":["./src/a.ts"]}}}"#,
  )
  .unwrap();
  cache.resolutions.borrow_mut().clear();
  assert!(build()["app.js"].contains("A_MARKER"));
}
