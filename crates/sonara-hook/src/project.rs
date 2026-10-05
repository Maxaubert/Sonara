//! The session's name (#245): the project it works in, not the folder its
//! shell happens to be in. Claude Code's `cwd` follows every `cd` (into
//! `app`, `node_modules\rwsdk\dist`, a worktree under `.claude\worktrees`),
//! so naming a session after `cwd`'s last folder renamed it all the time.
//!
//! The rule (`project_label`):
//! 1. A path inside `<repo>\.claude\worktrees\<name>` (Claude Code's own
//!    worktrees) is `<repo>`, from the path alone (no disk access).
//! 2. Else the nearest folder at or above `cwd` that holds `.git` (at most
//!    `MAX_DEPTH` levels, and never the user's home folder or above it: a
//!    dotfiles repository in the home would name every session after the
//!    user): a `.git` folder with a `HEAD` names its repository. A `.git`
//!    file (a linked worktree, `gitdir: <main>\.git\worktrees\<name>`) is
//!    followed to the main repository: the folder above the common git
//!    folder (`commondir`, else the `...\.git\worktrees\<name>` shape); a
//!    bare common folder `x.git` is `x`. A `.git` file without a common
//!    folder (a submodule) names its own folder.
//! 3. Outside any repository: `cwd`'s last folder. (The runtime keeps the
//!    first label a session got, so the folder it started in stays.)
//!
//! Only file system metadata and two small files are read; no process is
//! started, every error falls back to the next step. A UNC path is not
//! walked (an offline share could block the hook), and the hook looks the
//! label up only for an event that sends a message.
use std::path::{Path, PathBuf};

/// How many folders the walk to `.git` looks at, `cwd` included.
pub const MAX_DEPTH: usize = 40;
/// The most of a `.git` or `commondir` file that is read.
const FILE_MAX: u64 = 4096;

/// The last path component (either separator), as `os.path.basename` on
/// Windows.
pub fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or("")
}

/// The session's label for `cwd` (the module docs); `None` for an empty
/// path. `home` (the user's profile folder) ends the walk to `.git`.
pub fn project_label(cwd: &str, home: Option<&str>) -> Option<String> {
    if cwd.trim().is_empty() {
        return None;
    }
    let name = claude_worktree_repo(cwd)
        .map(str::to_string)
        .or_else(|| repo_name(Path::new(cwd), home))
        .unwrap_or_else(|| basename(cwd).to_string());
    Some(name).filter(|n| !n.is_empty())
}

/// Rule 1: the folder above `.claude` in `...\<repo>\.claude\worktrees\<name>`,
/// the first such part: a worktree made inside another one names the outer
/// repository.
fn claude_worktree_repo(cwd: &str) -> Option<&str> {
    let parts: Vec<&str> = cwd.split(['/', '\\']).collect();
    let i = parts.windows(3).position(|w| {
        w[0].eq_ignore_ascii_case(".claude")
            && w[1].eq_ignore_ascii_case("worktrees")
            && !w[2].is_empty()
    })?;
    let repo = *parts[..i].last()?;
    // A drive (`C:`) or an empty part (a root) is no repository name.
    Some(repo).filter(|r| !r.is_empty() && !r.ends_with(':'))
}

/// A path for comparing folders: one separator, no trailing one, case
/// ignored (Windows).
fn same_folder(a: &Path, b: &str) -> bool {
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    !b.trim().is_empty() && norm(&a.to_string_lossy()) == norm(b)
}

/// Rule 2: the repository the nearest `.git` at or above `dir` (below
/// `home`) belongs to.
fn repo_name(dir: &Path, home: Option<&str>) -> Option<String> {
    if is_network(dir) {
        return None;
    }
    for d in dir.ancestors().take(MAX_DEPTH) {
        if home.is_some_and(|h| same_folder(d, h)) {
            break;
        }
        let git = d.join(".git");
        let Ok(meta) = std::fs::metadata(&git) else {
            continue;
        };
        let own = name_of(d);
        if meta.is_dir() {
            // Git needs a HEAD; a stray `.git` folder is no repository.
            if git.join("HEAD").is_file() {
                return own;
            }
            continue;
        }
        if meta.is_file() {
            return linked_repo(d, &git).or(own);
        }
    }
    None
}

/// A UNC path (`\\server\share`, `//server/share`): no walk there, since
/// each lookup on an unreachable share can wait for the network timeout and
/// hold up the hook past its time budget. (A mapped drive letter is not
/// detected: that needs a Win32 call this crate does not make.)
fn is_network(dir: &Path) -> bool {
    let s = dir.to_string_lossy();
    let b = s.as_bytes();
    b.len() >= 2 && matches!(b[0], b'\\' | b'/') && matches!(b[1], b'\\' | b'/')
}

/// A folder's own name (`None` for a root).
fn name_of(d: &Path) -> Option<String> {
    d.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
}

/// Read at most `FILE_MAX` bytes of a small text file.
fn read_small(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut s = String::new();
    std::fs::File::open(p)
        .ok()?
        .take(FILE_MAX)
        .read_to_string(&mut s)
        .ok()?;
    Some(s)
}

/// A path written in a git file, relative to `base` unless absolute.
fn resolve(base: &Path, written: &str) -> PathBuf {
    let p = Path::new(written.trim());
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// The main repository's name for a `.git` file in `worktree`.
fn linked_repo(worktree: &Path, git_file: &Path) -> Option<String> {
    let text = read_small(git_file)?;
    let gitdir = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("gitdir:"))
        .map(|g| resolve(worktree, g))?;
    let common = match read_small(&gitdir.join("commondir")) {
        Some(c) if !c.trim().is_empty() => resolve(&gitdir, c.lines().next().unwrap_or("")),
        // `<main>\.git\worktrees\<name>` without a readable `commondir`.
        _ => {
            let parent = gitdir.parent()?;
            if !name_is(parent, "worktrees") {
                return None; // a submodule: its own name
            }
            parent.parent()?.to_path_buf()
        }
    };
    common_repo_name(&common)
}

fn name_is(p: &Path, name: &str) -> bool {
    p.file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
}

/// The repository a common git folder belongs to: the folder above `.git`,
/// or a bare `x.git`'s `x`.
fn common_repo_name(common: &Path) -> Option<String> {
    // `..` and `.` parts (commondir is usually `../..`) are dropped by
    // walking the components.
    let mut clean = PathBuf::new();
    for c in common.components() {
        match c {
            std::path::Component::ParentDir => {
                clean.pop();
            }
            std::path::Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let name = clean.file_name()?.to_string_lossy().into_owned();
    if name.eq_ignore_ascii_case(".git") {
        return clean.parent().and_then(name_of);
    }
    let bare = name.strip_suffix(".git").unwrap_or(&name);
    Some(bare.to_string()).filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir()
                .join(format!("sonara-hook-project-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn pl(cwd: &str) -> Option<String> {
        project_label(cwd, None)
    }

    /// A repository's `.git` folder (with the `HEAD` git needs).
    fn git_dir(repo: &Path) -> PathBuf {
        let git = repo.join(".git");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main").unwrap();
        git
    }

    #[test]
    fn a_subfolder_of_a_repo_is_named_after_the_repo() {
        let t = Tmp::new("sub");
        let repo = t.0.join("web-application-project");
        let deep = repo.join("node_modules").join("rwsdk").join("dist");
        git_dir(&repo);
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir_all(repo.join("app")).unwrap();
        for d in [&repo, &repo.join("app"), &deep] {
            assert_eq!(pl(&s(d)).as_deref(), Some("web-application-project"));
        }
        // A folder that is gone (the session deleted it) still finds it.
        assert_eq!(
            pl(&s(&repo.join("gone").join("auth"))).as_deref(),
            Some("web-application-project")
        );
    }

    #[test]
    fn a_linked_worktree_is_named_after_its_main_repo() {
        let t = Tmp::new("linked");
        let main = t.0.join("PrismTerminal");
        let admin = main.join(".git").join("worktrees").join("agent-hooks");
        fs::create_dir_all(&admin).unwrap();
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        let wt = t.0.join("elsewhere").join("agent-hooks");
        fs::create_dir_all(wt.join("src")).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", s(&admin))).unwrap();
        assert_eq!(pl(&s(&wt)).as_deref(), Some("PrismTerminal"));
        assert_eq!(pl(&s(&wt.join("src"))).as_deref(), Some("PrismTerminal"));
        // Without `commondir`: the `.git\worktrees\<name>` shape.
        fs::remove_file(admin.join("commondir")).unwrap();
        assert_eq!(pl(&s(&wt)).as_deref(), Some("PrismTerminal"));
        // A relative gitdir.
        let rel = main.join("nested-wt");
        fs::create_dir_all(&rel).unwrap();
        fs::write(rel.join(".git"), "gitdir: ../.git/worktrees/agent-hooks\n").unwrap();
        assert_eq!(pl(&s(&rel)).as_deref(), Some("PrismTerminal"));
    }

    #[test]
    fn a_bare_common_dir_and_a_submodule() {
        let t = Tmp::new("bare");
        let admin = t.0.join("tool.git").join("worktrees").join("w");
        fs::create_dir_all(&admin).unwrap();
        fs::write(admin.join("commondir"), "../..").unwrap();
        let wt = t.0.join("w");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}", s(&admin))).unwrap();
        assert_eq!(pl(&s(&wt)).as_deref(), Some("tool"));
        // A submodule's .git file points into the parent's modules: its own name.
        let sup = t.0.join("super");
        let modules = sup.join(".git").join("modules").join("lib");
        fs::create_dir_all(&modules).unwrap();
        let sub = sup.join("lib");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join(".git"), "gitdir: ../.git/modules/lib").unwrap();
        assert_eq!(pl(&s(&sub)).as_deref(), Some("lib"));
        // An unreadable or odd .git file: the folder that holds it.
        let odd = t.0.join("odd");
        fs::create_dir_all(&odd).unwrap();
        fs::write(odd.join(".git"), "nonsense").unwrap();
        assert_eq!(pl(&s(&odd)).as_deref(), Some("odd"));
    }

    #[test]
    fn a_claude_worktree_folder_is_named_after_the_repo() {
        // From the path alone: nothing of it exists.
        for cwd in [
            r"C:\nowhere-245\Filesmith\.claude\worktrees\statusbar",
            r"C:\nowhere-245\Sonara\.claude\worktrees\settings-redesign\crates\sonarad",
            "/nowhere-245/Filesmith/.claude/worktrees/app-icon/",
            r"\\server\share\Filesmith\.Claude\Worktrees\x",
            // A worktree made from inside another one: the outermost repo.
            r"C:\nowhere-245\Filesmith\.claude\worktrees\a\.claude\worktrees\b\src",
        ] {
            let want = if cwd.contains("Sonara") {
                "Sonara"
            } else {
                "Filesmith"
            };
            assert_eq!(pl(cwd).as_deref(), Some(want), "{cwd}");
        }
        // And with a worktree on disk that git also knows.
        let t = Tmp::new("claudewt");
        let repo = t.0.join("Filesmith");
        let wt = repo.join(".claude").join("worktrees").join("statusbar");
        fs::create_dir_all(repo.join(".git").join("worktrees").join("statusbar")).unwrap();
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../../../.git/worktrees/statusbar").unwrap();
        assert_eq!(pl(&s(&wt)).as_deref(), Some("Filesmith"));
    }

    #[test]
    fn a_folder_outside_a_repo_keeps_its_name() {
        let t = Tmp::new("plain");
        let d = t.0.join("notes");
        fs::create_dir_all(&d).unwrap();
        // Unless the temp folder itself sits in a repository (never on CI).
        if repo_name(&t.0, None).is_none() {
            assert_eq!(pl(&s(&d)).as_deref(), Some("notes"));
        }
        assert_eq!(
            pl(r"Q:\no-such-drive-245\work\proj").as_deref(),
            Some("proj")
        );
        assert_eq!(pl("/x/proj/").as_deref(), Some("proj"));
        // A network share is not walked (an offline one would block).
        assert!(is_network(Path::new(r"\\server\share\proj")));
        assert!(is_network(Path::new("//server/share/proj")));
        assert!(!is_network(Path::new(r"C:\proj")));
        assert!(!is_network(Path::new("/x/proj")));
        assert_eq!(pl(r"\\server-245\share\proj\src").as_deref(), Some("src"));
        // A stray `.git` folder without a HEAD is no repository.
        let stray = t.0.join("stray");
        fs::create_dir_all(stray.join(".git").join("info")).unwrap();
        fs::create_dir_all(stray.join("notes")).unwrap();
        if repo_name(&t.0, None).is_none() {
            assert_eq!(pl(&s(&stray.join("notes"))).as_deref(), Some("notes"));
        }
        assert_eq!(pl(""), None);
        assert_eq!(pl("   "), None);
        // A drive root has no folder name: what basename gives.
        assert_eq!(pl(r"Q:\").as_deref(), Some("Q:"));
        assert_eq!(pl(r"Q:\.claude\worktrees\x").as_deref(), Some("x"));
    }

    #[test]
    fn the_home_folder_is_never_the_project() {
        // A dotfiles repository in the user's home would name every
        // session there after the user.
        let t = Tmp::new("home");
        let home = t.0.join("Admin");
        git_dir(&home);
        let d = home.join("notes").join("drafts");
        fs::create_dir_all(&d).unwrap();
        assert_eq!(pl(&s(&d)).as_deref(), Some("Admin"), "without a home");
        assert_eq!(
            project_label(&s(&d), Some(&s(&home))).as_deref(),
            Some("drafts")
        );
        // Any spelling of the home path.
        let upper = s(&home).to_uppercase().replace('\\', "/") + "/";
        assert_eq!(
            project_label(&s(&d), Some(&upper)).as_deref(),
            Some("drafts")
        );
        // A repository below the home still names it.
        let repo = home.join("code").join("Wind");
        git_dir(&repo);
        fs::create_dir_all(repo.join("src")).unwrap();
        assert_eq!(
            project_label(&s(&repo.join("src")), Some(&s(&home))).as_deref(),
            Some("Wind")
        );
    }
}
