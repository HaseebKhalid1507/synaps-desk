//! Files a `bash` tool call wrote, read from its command text: output
//! redirects (`>`, `>>`, `&>`), `tee`, `sed -i`, the targets of `mv`, `cp`,
//! `rm`, `touch` and `mkdir`, `git checkout -- <paths>`, `git restore` and
//! a `git apply` of a here-document. No GPUI, no shell is run.
//!
//! Conservative: a command this cannot read for certain (command or
//! process substitution it cannot match, arithmetic, `case`, an
//! unterminated quote or here-document) gives nothing at all, and a
//! target it cannot pin down (an expansion, a glob, a folder after an
//! unknown `cd`) is left out, never guessed. What remains is still an
//! approximation: a branch of an `if` or `||` that did not run is counted,
//! and `mv`/`cp` to an existing folder name the folder.

/// One file (or folder) a command wrote, moved, removed or created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Touch {
    /// As written in the command (`~` kept), joined to the folder of a
    /// literal `cd` before it.
    pub path: String,
    /// How: `>`, `>>`, `tee`, `sed -i`, `mv from`, `mv to`, `cp to`, `rm`,
    /// `touch`, `mkdir`, `checkout`, `restore`, `apply`.
    pub verb: &'static str,
}

/// The files `command` writes, in order; `None` when it cannot be read.
pub fn writes(command: &str) -> Option<Vec<Touch>> {
    let (toks, bodies) = lex(command)?;
    let mut out = Vec::new();
    let mut bodies = bodies.into_iter();
    let mut base = Base::Cwd;
    let mut stack: Vec<Base> = Vec::new();
    let mut cmd: Vec<Tok> = Vec::new();
    for t in toks.into_iter().chain([Tok::Op(";")]) {
        match t {
            Tok::Op(op @ (";" | "\n" | "&&" | "||" | "|" | "|&" | "&" | "(" | ")")) => {
                simple(std::mem::take(&mut cmd), &mut base, &mut bodies, &mut out)?;
                match op {
                    "(" => stack.push(base.clone()),
                    ")" => base = stack.pop()?,
                    _ => {}
                }
            }
            t => cmd.push(t),
        }
    }
    Some(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Word {
    text: String,
    /// The text is what the shell passes on: no expansion, no glob.
    literal: bool,
    /// An unquoted expansion or glob: it may become no word or several.
    splits: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Word(Word),
    /// A separator, or a redirect operator (its fd prefix dropped).
    Op(&'static str),
}

/// The folder relative paths resolve against.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Base {
    /// The session's folder (paths stay as written).
    Cwd,
    Dir(String),
    /// After a `cd` this could not follow: relative targets are skipped.
    Unknown,
}

const REDIRECTS: [&str; 11] = [">>", ">|", ">&", ">", "<<<", "<<-", "<<", "<>", "<&", "<", "&>>"];

/// Words and operators, and the here-document bodies in order. `None`:
/// something this lexer does not follow.
fn lex(s: &str) -> Option<(Vec<Tok>, Vec<String>)> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut toks = Vec::new();
    let mut bodies = Vec::new();
    // Here-documents opened on this line: (delimiter, strip tabs).
    let mut pending: Vec<(String, bool)> = Vec::new();
    let mut want_delim: Option<bool> = None;
    while i < c.len() {
        match c[i] {
            ' ' | '\t' => i += 1,
            '\\' if c.get(i + 1) == Some(&'\n') => i += 2,
            '\n' => {
                toks.push(Tok::Op("\n"));
                i += 1;
                if want_delim.is_some() {
                    return None;
                }
                for (delim, strip) in pending.drain(..) {
                    let mut body = String::new();
                    loop {
                        if i >= c.len() {
                            return None;
                        }
                        let end = c[i..].iter().position(|&x| x == '\n').map_or(c.len(), |n| i + n);
                        let line: String = c[i..end].iter().collect();
                        i = (end + 1).min(c.len());
                        let l = if strip { line.trim_start_matches('\t') } else { &line };
                        if l == delim {
                            break;
                        }
                        body.push_str(l);
                        body.push('\n');
                    }
                    bodies.push(body);
                }
            }
            '#' => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            ';' if c.get(i + 1) == Some(&';') => return None,
            ';' => {
                toks.push(Tok::Op(";"));
                i += 1;
            }
            '(' if c.get(i + 1) == Some(&'(') => return None,
            '(' | ')' => {
                toks.push(Tok::Op(if c[i] == '(' { "(" } else { ")" }));
                i += 1;
            }
            '&' | '|' | '<' | '>' => {
                let rest: String = c[i..(i + 3).min(c.len())].iter().collect();
                if rest.starts_with("<(") || rest.starts_with(">(") {
                    return None;
                }
                let op = ["&&", "&>>", "&>", "&", "||", "|&", "|"]
                    .into_iter()
                    .chain(REDIRECTS)
                    .filter(|o| rest.starts_with(o))
                    .max_by_key(|o| o.len())?;
                i += op.len();
                if want_delim.is_some() {
                    return None;
                }
                if op == "<<" || op == "<<-" {
                    want_delim = Some(op == "<<-");
                }
                toks.push(Tok::Op(op));
            }
            _ => {
                let (w, next) = word(&c, i)?;
                i = next;
                // `2>file`, `2>&1`: an fd number glued to a redirect.
                if w.literal && !w.text.is_empty() && w.text.bytes().all(|b| b.is_ascii_digit()) && matches!(c.get(i), Some('<' | '>')) {
                    continue;
                }
                if let Some(strip) = want_delim.take() {
                    pending.push((w.text, strip));
                    continue;
                }
                toks.push(Tok::Word(w));
            }
        }
    }
    // A here-document with no body, or no delimiter: not read.
    if !pending.is_empty() || want_delim.is_some() {
        return None;
    }
    Some((toks, bodies))
}

/// One word from `c[i]`; returns it and the index after it.
fn word(c: &[char], mut i: usize) -> Option<(Word, usize)> {
    let mut w = Word { text: String::new(), literal: true, splits: false };
    let start = i;
    while i < c.len() {
        match c[i] {
            ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')' | '<' | '>' => break,
            '\\' => {
                match c.get(i + 1) {
                    Some('\n') => {}
                    Some(&x) => w.text.push(x),
                    None => {}
                }
                i += 2;
            }
            '\'' => {
                let end = c[i + 1..].iter().position(|&x| x == '\'')? + i + 1;
                w.text.extend(&c[i + 1..end]);
                i = end + 1;
            }
            '"' => {
                i += 1;
                loop {
                    match *c.get(i)? {
                        '"' => break,
                        '\\' if matches!(c.get(i + 1), Some('$' | '`' | '"' | '\\' | '\n')) => {
                            if c[i + 1] != '\n' {
                                w.text.push(c[i + 1]);
                            }
                            i += 2;
                        }
                        '$' | '`' => i = expansion(c, i, &mut w)?.0,
                        x => {
                            w.text.push(x);
                            i += 1;
                        }
                    }
                }
                i += 1;
            }
            '$' | '`' => {
                let (next, expanded) = expansion(c, i, &mut w)?;
                i = next;
                w.splits |= expanded;
            }
            '*' | '?' | '[' | '{' if !(c[i] == '{' && i == start && matches!(c.get(i + 1), None | Some(' ' | '\t' | '\n'))) => {
                w.text.push(c[i]);
                w.literal = false;
                w.splits = true;
                i += 1;
            }
            '~' if i == start && !matches!(c.get(i + 1), None | Some('/' | ' ' | '\t' | '\n' | ';' | '&' | '|' | ')')) => {
                // `~user`: another user's home, not followed.
                w.text.push('~');
                w.literal = false;
                i += 1;
            }
            x => {
                w.text.push(x);
                i += 1;
            }
        }
    }
    Some((w, i))
}

/// Skip a `$…` or `` `…` `` at `c[i]`, marking `w` not literal (a lone `$`
/// is the character). Returns the index after it and whether it expanded.
fn expansion(c: &[char], i: usize, w: &mut Word) -> Option<(usize, bool)> {
    if c[i] == '`' {
        let mut j = i + 1;
        loop {
            match *c.get(j)? {
                '\\' => j += 2,
                '`' => break,
                _ => j += 1,
            }
        }
        w.literal = false;
        return Some((j + 1, true));
    }
    let next = c.get(i + 1).copied();
    let end = match next {
        Some('(') => matching(c, i + 1, '(', ')')?,
        Some('{') => matching(c, i + 1, '{', '}')?,
        Some('\'') => {
            // `$'…'`: ANSI-C quoting, `\'` inside.
            let mut j = i + 2;
            loop {
                match *c.get(j)? {
                    '\\' => j += 2,
                    '\'' => break,
                    _ => j += 1,
                }
            }
            j + 1
        }
        Some(x) if x.is_ascii_alphabetic() || x == '_' => {
            let mut j = i + 1;
            while c.get(j).is_some_and(|x| x.is_ascii_alphanumeric() || *x == '_') {
                j += 1;
            }
            j
        }
        Some(x) if x.is_ascii_digit() || "@*#?$!-".contains(x) => i + 2,
        _ => {
            w.text.push('$');
            return Some((i + 1, false));
        }
    };
    w.literal = false;
    Some((end, true))
}

/// The index after the `close` matching the `open` at `c[i]`, past quotes.
fn matching(c: &[char], mut i: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    while i < c.len() {
        match c[i] {
            '\\' => i += 1,
            '\'' => i += c[i + 1..].iter().position(|&x| x == '\'')? + 1,
            '"' => {
                i += 1;
                while *c.get(i)? != '"' {
                    i += if c[i] == '\\' { 2 } else { 1 };
                }
            }
            x if x == open => depth += 1,
            x if x == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The folder-joined path of a literal target, if it can be named.
fn resolve(base: &Base, w: &Word) -> Option<String> {
    if !w.literal || w.text.is_empty() || w.text == "-" {
        return None;
    }
    let p = w.text.as_str();
    if p.starts_with("/dev/") || p == "/dev" {
        return None;
    }
    let joined = if p.starts_with('/') || p.starts_with('~') {
        p.to_string()
    } else {
        match base {
            Base::Cwd => p.to_string(),
            Base::Dir(b) => format!("{}/{p}", b.trim_end_matches('/')),
            Base::Unknown => return None,
        }
    };
    Some(normalize(&joined))
}

/// `./a//b/./c/` → `a/b/c` (no `..` folding: a symlink may sit there).
fn normalize(p: &str) -> String {
    let abs = p.starts_with('/');
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let joined = parts.join("/");
    match (abs, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

fn basename(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

/// One simple command: its redirects' targets, then its own.
fn simple(toks: Vec<Tok>, base: &mut Base, bodies: &mut impl Iterator<Item = String>, out: &mut Vec<Touch>) -> Option<()> {
    let mut words: Vec<Word> = Vec::new();
    let mut heredoc: Option<String> = None;
    let mut redirects: Vec<(&'static str, Word)> = Vec::new();
    let mut it = toks.into_iter();
    while let Some(t) = it.next() {
        match t {
            Tok::Word(w) => words.push(w),
            Tok::Op(op) => {
                if op == "<<" || op == "<<-" {
                    heredoc = Some(bodies.next()?);
                    continue;
                }
                let Some(Tok::Word(target)) = it.next() else { return None };
                redirects.push((op, target));
            }
        }
    }
    // `[[ a > b ]]` compares; nothing is written.
    if words.first().is_some_and(|w| w.text == "[[") {
        return Some(());
    }
    for (op, target) in &redirects {
        let verb = match *op {
            ">" | ">|" | "&>" => ">",
            ">>" | "&>>" => ">>",
            _ => continue,
        };
        if let Some(path) = resolve(base, target) {
            out.push(Touch { path, verb });
        }
    }
    // Assignments, wrappers and keywords before the command.
    let mut args = words.as_slice();
    while let Some((w, rest)) = args.split_first() {
        let assignment = w.text.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        });
        let wrapper = w.literal
            && matches!(
                w.text.as_str(),
                "!" | "{" | "}" | "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "time" | "nohup" | "command" | "builtin"
            );
        if assignment || wrapper {
            args = rest;
            continue;
        }
        if w.literal && (w.text == "sudo" || w.text == "env") {
            if rest.first().is_some_and(|w| w.text.starts_with('-')) {
                return Some(());
            }
            args = rest;
            continue;
        }
        break;
    }
    let Some((cmd, args)) = args.split_first() else { return Some(()) };
    if !cmd.literal {
        return Some(());
    }
    let name = basename(&cmd.text);
    match name {
        "cd" | "pushd" => {
            *base = match args {
                [] => Base::Dir("~".into()),
                [d] if !d.text.starts_with('-') => match resolve(base, d) {
                    Some(p) => Base::Dir(p),
                    None => Base::Unknown,
                },
                _ => Base::Unknown,
            };
        }
        "popd" => *base = Base::Unknown,
        "for" | "select" | "function" => {}
        "case" | "coproc" => return None,
        "rm" | "rmdir" | "unlink" => {
            let (_, files) = options(args, &[], &[])?;
            push_all(&files, base, "rm", out);
        }
        "touch" => {
            let (_, files) = options(args, &["d", "t", "r"], &["date", "reference"])?;
            push_all(&files, base, "touch", out);
        }
        "mkdir" => {
            let (_, files) = options(args, &["m"], &["mode"])?;
            push_all(&files, base, "mkdir", out);
        }
        "tee" => {
            let (flags, files) = options(args, &[], &[])?;
            let verb = if flags.iter().any(|f| f == "a" || f == "append") { "tee -a" } else { "tee" };
            push_all(&files, base, verb, out);
        }
        "sed" => sed(args, base, out)?,
        "mv" | "cp" => move_or_copy(name == "mv", args, base, out)?,
        "git" => git(args, base, heredoc.as_deref(), out)?,
        _ => {}
    }
    Some(())
}

fn push_all(files: &[&Word], base: &Base, verb: &'static str, out: &mut Vec<Touch>) {
    out.extend(files.iter().filter_map(|w| resolve(base, w)).map(|path| Touch { path, verb }));
}

/// Split `args` into option names (`-ab` → `a`, `b`; `--x=y` → `x`) and the
/// rest. `with_arg`: short and long options that take the next word.
/// `None`: an option given by an expansion (it could be anything).
fn options<'a>(args: &'a [Word], short_arg: &[&str], long_arg: &[&str]) -> Option<(Vec<String>, Vec<&'a Word>)> {
    let mut flags = Vec::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(w) = it.next() {
        if w.text == "--" && w.literal {
            rest.extend(it.by_ref());
            break;
        }
        if !w.text.starts_with('-') || w.text == "-" {
            rest.push(w);
            continue;
        }
        if !w.literal {
            return None;
        }
        if let Some(long) = w.text.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((n, _)) => (n, true),
                None => (long, false),
            };
            if long_arg.contains(&name) && !value {
                it.next()?;
            }
            flags.push(name.to_string());
            continue;
        }
        let chars: Vec<char> = w.text[1..].chars().collect();
        for (k, ch) in chars.iter().enumerate() {
            let s = ch.to_string();
            let takes = short_arg.contains(&s.as_str());
            flags.push(s);
            if takes {
                if k + 1 == chars.len() {
                    it.next()?;
                }
                break;
            }
        }
    }
    Some((flags, rest))
}

/// `sed -i[SUFFIX]`: the files after the script (no `-i`: nothing written).
fn sed(args: &[Word], base: &Base, out: &mut Vec<Touch>) -> Option<()> {
    let mut in_place = false;
    let mut script_given = false;
    let mut positional: Vec<&Word> = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(w) = it.next() {
        let t = w.text.as_str();
        if t == "--" && w.literal {
            positional.extend(it.by_ref());
            break;
        }
        if !t.starts_with('-') || t == "-" {
            positional.push(w);
            continue;
        }
        if !w.literal {
            return None;
        }
        if let Some(long) = t.strip_prefix("--") {
            let name = long.split_once('=').map_or(long, |(n, _)| n);
            match name {
                "in-place" => in_place = true,
                "expression" | "file" => {
                    script_given = true;
                    if !long.contains('=') {
                        it.next()?;
                    }
                }
                "line-length" if !long.contains('=') => {
                    it.next()?;
                }
                "regexp-extended" | "quiet" | "silent" | "separate" | "null-data" | "zero-terminated" | "posix" | "sandbox"
                | "follow-symlinks" | "unbuffered" | "debug" | "line-length" => {}
                _ => return None,
            }
            continue;
        }
        // A cluster: `-i` takes the rest as its suffix; `-e`, `-f`, `-l`
        // the rest or the next word.
        let chars: Vec<char> = t[1..].chars().collect();
        for (k, ch) in chars.iter().enumerate() {
            match ch {
                'i' => {
                    in_place = true;
                    // BSD sed: `-i ''` (the suffix as its own, empty, word).
                    if k + 1 == chars.len() && it.peek().is_some_and(|n| n.literal && n.text.is_empty()) {
                        it.next();
                    }
                    break;
                }
                'e' | 'f' | 'l' => {
                    if *ch != 'l' {
                        script_given = true;
                    }
                    if k + 1 == chars.len() {
                        it.next()?;
                    }
                    break;
                }
                'n' | 'r' | 'E' | 's' | 'u' | 'z' => {}
                _ => return None,
            }
        }
    }
    if !in_place {
        return Some(());
    }
    let files = if script_given {
        &positional[..]
    } else {
        let (script, files) = positional.split_first()?;
        if script.splits {
            return None;
        }
        files
    };
    push_all(files, base, "sed -i", out);
    Some(())
}

/// `mv` / `cp`: the destination (a file, or each source's name in a folder
/// given with `-t`, a trailing `/` or several sources); `mv` also its sources.
fn move_or_copy(mv: bool, args: &[Word], base: &Base, out: &mut Vec<Touch>) -> Option<()> {
    let known: &[&str] = &[
        "f",
        "i",
        "n",
        "v",
        "u",
        "b",
        "T",
        "r",
        "R",
        "a",
        "p",
        "l",
        "s",
        "L",
        "P",
        "H",
        "d",
        "x",
        "force",
        "interactive",
        "no-clobber",
        "verbose",
        "update",
        "backup",
        "no-target-directory",
        "recursive",
        "archive",
        "preserve",
        "no-preserve",
        "reflink",
        "link",
        "symbolic-link",
        "dereference",
        "no-dereference",
        "one-file-system",
        "sparse",
        "strip-trailing-slashes",
        "target-directory",
        "suffix",
        "t",
        "S",
        "attributes-only",
        "remove-destination",
        "exchange",
        "debug",
        "keep-directory-symlink",
    ];
    // `-t DIR` / `--target-directory=DIR` name the folder: find it first.
    let mut target_dir: Option<&Word> = None;
    let mut cleaned: Vec<&Word> = Vec::new();
    let mut it = args.iter();
    let mut no_target_dir = false;
    let mut after_dashes = false;
    while let Some(w) = it.next() {
        let t = w.text.as_str();
        if after_dashes || !t.starts_with('-') || t == "-" {
            cleaned.push(w);
            continue;
        }
        if t == "--" {
            after_dashes = true;
            continue;
        }
        if !w.literal {
            return None;
        }
        if let Some(long) = t.strip_prefix("--") {
            let (name, value) = long.split_once('=').map_or((long, None), |(n, v)| (n, Some(v)));
            if !known.contains(&name) || name == "parents" {
                return None;
            }
            match name {
                "target-directory" => {
                    if value.is_some() {
                        return None;
                    }
                    target_dir = Some(it.next()?);
                }
                "suffix" if value.is_none() => {
                    it.next()?;
                }
                "no-target-directory" => no_target_dir = true,
                _ => {}
            }
            continue;
        }
        let chars: Vec<char> = t[1..].chars().collect();
        for (k, ch) in chars.iter().enumerate() {
            let s = ch.to_string();
            if !known.contains(&s.as_str()) {
                return None;
            }
            match ch {
                'T' => no_target_dir = true,
                't' | 'S' => {
                    if k + 1 != chars.len() {
                        return None;
                    }
                    let v = it.next()?;
                    if *ch == 't' {
                        target_dir = Some(v);
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    // A word that may split shifts which one is the destination.
    if cleaned.iter().any(|w| w.splits) || target_dir.is_some_and(|d| d.splits) {
        return None;
    }
    let (sources, dir, dest): (&[&Word], Option<&Word>, Option<&Word>) = match target_dir {
        Some(d) => (&cleaned[..], Some(d), None),
        None => {
            let (dest, sources) = cleaned.split_last()?;
            if sources.is_empty() {
                return None;
            }
            let into_dir = !no_target_dir && (sources.len() > 1 || dest.text.ends_with('/'));
            if into_dir { (sources, Some(*dest), None) } else { (sources, None, Some(*dest)) }
        }
    };
    let to = if mv { "mv to" } else { "cp to" };
    for src in sources {
        if mv && let Some(path) = resolve(base, src) {
            out.push(Touch { path, verb: "mv from" });
        }
        if let Some(d) = dir
            && src.literal
        {
            let inside = Word { text: format!("{}/{}", d.text.trim_end_matches('/'), basename(&src.text)), ..d.clone() };
            if let Some(path) = resolve(base, &inside) {
                out.push(Touch { path, verb: to });
            }
        }
    }
    if let Some(d) = dest
        && let Some(path) = resolve(base, d)
    {
        out.push(Touch { path, verb: to });
    }
    Some(())
}

/// `git checkout [<ref>] -- <paths>`, `git restore <paths>` (not
/// `--staged` alone), `git apply` of a here-document (`+++ b/<path>`).
fn git(args: &[Word], base: &Base, heredoc: Option<&str>, out: &mut Vec<Touch>) -> Option<()> {
    let (sub, rest) = args.split_first()?;
    if !sub.literal {
        return Some(());
    }
    match sub.text.as_str() {
        "checkout" => {
            let Some(i) = rest.iter().position(|w| w.literal && w.text == "--") else { return Some(()) };
            if rest[..i].iter().any(|w| !w.literal || w.text == "-p" || w.text == "--patch") {
                return Some(());
            }
            push_all(&rest[i + 1..].iter().collect::<Vec<_>>(), base, "checkout", out);
        }
        "restore" => {
            let (flags, paths) = options(rest, &["s"], &["source"])?;
            let staged = flags.iter().any(|f| f == "S" || f == "staged");
            let worktree = flags.iter().any(|f| f == "W" || f == "worktree");
            if flags.iter().any(|f| f == "p" || f == "patch") || (staged && !worktree) {
                return Some(());
            }
            push_all(&paths, base, "restore", out);
        }
        "apply" => {
            let (flags, files) = options(rest, &["C"], &["exclude", "include", "directory"])?;
            let reads_only = ["check", "stat", "numstat", "summary", "cached"];
            if flags.iter().any(|f| reads_only.contains(&f.as_str())) && !flags.iter().any(|f| f == "apply" || f == "index") {
                return Some(());
            }
            // Only a here-document's patch can be read; `-p<n>` and
            // `--directory` move its paths.
            if flags.iter().any(|f| f == "p" || f == "directory" || f == "include" || f == "exclude") || !files.is_empty() {
                return Some(());
            }
            let Some(patch) = heredoc else { return Some(()) };
            let mut old: Option<&str> = None;
            for line in patch.lines() {
                if let Some(a) = line.strip_prefix("--- ") {
                    old = Some(a.split('\t').next().unwrap_or(a));
                } else if let Some(b) = line.strip_prefix("+++ ") {
                    let b = b.split('\t').next().unwrap_or(b);
                    let path = if b == "/dev/null" { old.and_then(|a| a.strip_prefix("a/")) } else { b.strip_prefix("b/") };
                    let w = Word { text: path?.to_string(), literal: true, splits: false };
                    if let Some(path) = resolve(base, &w) {
                        out.push(Touch { path, verb: "apply" });
                    }
                }
            }
        }
        _ => {}
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(cmd: &str) -> Option<Vec<(String, &'static str)>> {
        writes(cmd).map(|v| v.into_iter().map(|t| (t.path, t.verb)).collect())
    }

    /// What a command writes: (path, verb) pairs, or `None` (not read).
    type Expect = Option<&'static [(&'static str, &'static str)]>;

    /// Commands as agents write them, and what they write. `None`: not read.
    #[test]
    fn real_commands() {
        let cases: &[(&str, Expect)] = &[
            // Redirects, on any command.
            ("echo hi > notes.txt", Some(&[("notes.txt", ">")])),
            ("printf '%s\\n' a b >> log/run.log 2>&1", Some(&[("log/run.log", ">>")])),
            ("cargo test 2>/dev/null >out.txt", Some(&[("out.txt", ">")])),
            ("python3 gen.py &> build.log", Some(&[("build.log", ">")])),
            ("ls -la", Some(&[])),
            ("grep -n foo src/*.rs | head -20", Some(&[])),
            ("cat > src/config.toml <<'EOF'\n[server]\nport = 8080 # > not a redirect\nEOF\necho done", Some(&[("src/config.toml", ">")])),
            ("cat <<-EOF > ./docs//a.md\n\t# title\n\tEOF", Some(&[("docs/a.md", ">")])),
            ("echo \"$HOME\" > \"$OUT\"", Some(&[])),
            ("echo x > out-$(date +%s).log", Some(&[])),
            ("[[ $a > $b ]] && echo yes", Some(&[])),
            // tee, sed -i.
            ("echo 'k=v' | sudo tee -a /etc/app.conf > /dev/null", Some(&[("/etc/app.conf", "tee -a")])),
            ("npm run build | tee build.log", Some(&[("build.log", "tee")])),
            ("sed -i 's/foo/bar/g' src/a.rs src/b.rs", Some(&[("src/a.rs", "sed -i"), ("src/b.rs", "sed -i")])),
            ("sed -i.bak -e 's/x/y/' -e '/^$/d' README.md", Some(&[("README.md", "sed -i")])),
            ("sed -Ei 's/(a)+/b/' 'my file.txt'", Some(&[("my file.txt", "sed -i")])),
            ("sed -i '' 's/a/b/' mac.txt", Some(&[("mac.txt", "sed -i")])),
            ("sed -n '1,20p' src/main.rs", Some(&[])),
            ("sed 's/a/b/' in.txt > out.txt", Some(&[("out.txt", ">")])),
            ("sed -i \"s/$OLD/$NEW/\" conf.ini", Some(&[("conf.ini", "sed -i")])),
            ("sed -i $SCRIPT conf.ini", None),
            // mv, cp, rm, touch, mkdir.
            ("mv old.rs new.rs", Some(&[("old.rs", "mv from"), ("new.rs", "mv to")])),
            (
                "mv a.txt b.txt archive/",
                Some(&[("a.txt", "mv from"), ("archive/a.txt", "mv to"), ("b.txt", "mv from"), ("archive/b.txt", "mv to")]),
            ),
            ("cp -r templates/base site/", Some(&[("site/base", "cp to")])),
            ("cp -t dist/ a.js b.js", Some(&[("dist/a.js", "cp to"), ("dist/b.js", "cp to")])),
            ("cp --parents a/b.txt out/", None),
            ("cp src/*.rs backup/", None),
            ("rm -rf target/ node_modules", Some(&[("target", "rm"), ("node_modules", "rm")])),
            ("rm -f -- -weird-name", Some(&[("-weird-name", "rm")])),
            ("rm *.o", Some(&[])),
            ("touch -d '2 days ago' stamp", Some(&[("stamp", "touch")])),
            (
                "mkdir -p crates/new/src && touch crates/new/src/lib.rs",
                Some(&[("crates/new/src", "mkdir"), ("crates/new/src/lib.rs", "touch")]),
            ),
            ("mkdir -m 700 secret", Some(&[("secret", "mkdir")])),
            // cd: relative targets follow a literal one, not an unknown one.
            ("cd /tmp/work && echo x > a.txt", Some(&[("/tmp/work/a.txt", ">")])),
            ("cd sub; rm b.txt", Some(&[("sub/b.txt", "rm")])),
            ("cd \"$(git rev-parse --show-toplevel)\" && rm b.txt /abs/c", Some(&[("/abs/c", "rm")])),
            ("(cd docs && touch x.md) && touch y.md", Some(&[("docs/x.md", "touch"), ("y.md", "touch")])),
            ("cd ~/proj && mkdir out", Some(&[("~/proj/out", "mkdir")])),
            // git.
            ("git checkout -- src/a.rs src/b.rs", Some(&[("src/a.rs", "checkout"), ("src/b.rs", "checkout")])),
            ("git checkout main -- Cargo.lock", Some(&[("Cargo.lock", "checkout")])),
            ("git checkout -b feature", Some(&[])),
            ("git checkout main", Some(&[])),
            ("git restore src/x.rs", Some(&[("src/x.rs", "restore")])),
            ("git restore --staged src/x.rs", Some(&[])),
            ("git restore --staged --worktree src/x.rs", Some(&[("src/x.rs", "restore")])),
            (
                "git apply <<'PATCH'\ndiff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/old.txt b/old.txt\n--- a/old.txt\n+++ /dev/null\nPATCH",
                Some(&[("src/a.rs", "apply"), ("old.txt", "apply")]),
            ),
            ("git apply fix.patch", Some(&[])),
            ("git apply --check <<'P'\n+++ b/x\nP", Some(&[])),
            ("git -C repo checkout -- a", Some(&[])),
            ("git status && git diff --stat", Some(&[])),
            // Loops and conditions: literal targets only.
            ("for f in *.md; do sed -i 's/a/b/' \"$f\"; done", Some(&[])),
            ("if [ -f x.lock ]; then rm x.lock; fi", Some(&[("x.lock", "rm")])),
            ("FOO=1 BAR=2 make > make.log", Some(&[("make.log", ">")])),
            // Not read at all.
            ("(( n > 3 )) && rm a", None),
            ("case $x in a) rm a;; esac", None),
            ("diff <(sort a) <(sort b) > d.txt", None),
            ("echo 'unterminated > x", None),
            ("cat > f <<EOF\nno end", None),
            ("echo $(echo ')' > x.txt", None),
            // Command substitution that closes is skipped over, not read.
            ("mkdir -p \"$(dirname out/a/b.txt)\" && echo hi > out/a/b.txt", Some(&[("out/a/b.txt", ">")])),
            ("echo `date` >> stamps.log", Some(&[("stamps.log", ">>")])),
        ];
        for (cmd, want) in cases {
            let want: Option<Vec<(String, &str)>> = want.map(|w| w.iter().map(|(p, v)| (p.to_string(), *v)).collect());
            assert_eq!(paths(cmd), want, "{cmd:?}");
        }
    }

    #[test]
    fn junk_does_not_panic() {
        for s in [
            "",
            "\\",
            "'",
            "\"",
            "$",
            "$(",
            "${",
            "`",
            "<<",
            "> ",
            "2>",
            "a\\\n> b",
            "{",
            "~",
            "~x/y > a",
            "cd",
            "mv",
            "cp a",
            "sed -i",
            "git",
            "git apply <<",
            "((",
            "a;;b",
            "<(",
            "|&",
            "&>>",
        ] {
            let _ = writes(s);
        }
    }
}
