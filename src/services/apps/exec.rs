use std::path::{Path, PathBuf};

/*
 * removes the field codes like %f and %U, which a launcher
 * fills with files to open, and a menu has none of
 */
pub fn strip_field_codes(exec: &str) -> String {
    let mut command = String::new();

    let mut characters = exec.chars();

    while let Some(character) = characters.next() {
        if character != '%' {
            command.push(character);

            continue;
        }

        // %% is a plain percent sign, anything else after % is dropped with it
        if let Some('%') = characters.next() {
            command.push('%');
        }
    }

    command.trim().to_string()
}

/*
 * the commands, as argument lists, that open `files` with a program run by `exec`: %f and %u
 * take one file each, so there's a command a file, %F and %U take them all; none of them, and
 * the program opens no files, so it's started plain.
 *
 * the Exec line is split into arguments as the Desktop Entry spec says (spaces between them,
 * double quotes around one, with \ escaping inside), and a file is always an argument of its
 * own, never text a shell reads: a file named `$(rm -rf ~).txt` stays only a name. None for
 * an Exec line that can't be split, like one with a quote left open
 */
pub fn fill_field_codes(exec: &str, files: &[PathBuf]) -> Option<Vec<Vec<String>>> {
    let arguments = split(exec)?;

    let one = arguments
        .iter()
        .any(|(argument, quoted)| !quoted && (argument.contains("%f") || argument.contains("%u")));

    if one && !files.is_empty() {
        return Some(
            files
                .iter()
                .map(|file| fill(&arguments, std::slice::from_ref(file)))
                .collect(),
        );
    }

    Some(vec![fill(&arguments, files)])
}

// an Exec line's arguments, each with whether it was quoted, which a field code can't be in
fn split(exec: &str) -> Option<Vec<(String, bool)>> {
    let mut arguments = Vec::new();

    let mut argument = String::new();
    let mut in_argument = false;
    let mut quoted = false;
    let mut in_quotes = false;

    let mut characters = exec.trim().chars();

    while let Some(character) = characters.next() {
        match character {
            ' ' | '\t' if !in_quotes => {
                if in_argument {
                    arguments.push((std::mem::take(&mut argument), quoted));
                    in_argument = false;
                    quoted = false;
                }
            }
            '"' => {
                in_quotes = !in_quotes;
                in_argument = true;
                quoted = true;
            }
            // inside quotes, \ takes the next letter as it is: \" \` \$ \\
            '\\' if in_quotes => argument.push(characters.next()?),
            _ => {
                argument.push(character);
                in_argument = true;
            }
        }
    }

    if in_quotes {
        return None;
    }

    if in_argument {
        arguments.push((argument, quoted));
    }

    (!arguments.is_empty()).then_some(arguments)
}

// the arguments with the field codes filled in with `files`
fn fill(arguments: &[(String, bool)], files: &[PathBuf]) -> Vec<String> {
    let paths = || files.iter().map(|file| file.to_string_lossy().into_owned());
    let links = || files.iter().map(|file| link(file));

    let mut filled = Vec::new();

    for (argument, quoted) in arguments {
        // the spec has no field codes inside quotes
        if *quoted {
            filled.push(argument.clone());
            continue;
        }

        // a code alone is the files, each an argument of its own
        match argument.as_str() {
            "%F" => {
                filled.extend(paths());
                continue;
            }
            "%U" => {
                filled.extend(links());
                continue;
            }
            "%f" => {
                filled.extend(paths().take(1));
                continue;
            }
            "%u" => {
                filled.extend(links().take(1));
                continue;
            }
            _ => {}
        }

        // a code inside an argument, like --file=%f, is one file in its place
        let mut text = String::new();
        let mut left_out = false;
        let mut characters = argument.chars();

        while let Some(character) = characters.next() {
            if character != '%' {
                text.push(character);
                continue;
            }

            match characters.next() {
                Some('%') => text.push('%'),
                Some('f' | 'F') => text.push_str(&paths().next().unwrap_or_default()),
                Some('u' | 'U') => text.push_str(&links().next().unwrap_or_default()),
                // the rest, like %i and %c, a launcher may leave out
                _ => left_out = true,
            }
        }

        // an argument that was only a code left out goes with it
        if !(left_out && text.is_empty()) {
            filled.push(text);
        }
    }

    filled
}

// a file:// link to `file`, with the letters a link can't hold escaped
fn link(file: &Path) -> String {
    let mut link = String::from("file://");

    for byte in file.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            link.push(byte as char);
        } else {
            link.push_str(&format!("%{byte:02X}"));
        }
    }

    link
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(exec: &str, files: &[PathBuf]) -> Vec<Vec<String>> {
        fill_field_codes(exec, files).expect("an Exec line that splits")
    }

    #[test]
    fn fills_files_in_for_their_field_codes() {
        let files = [
            PathBuf::from("/home/me/a b.md"),
            PathBuf::from("/tmp/it's.txt"),
        ];

        assert_eq!(
            argv("code --new-window %F", &files),
            [["code", "--new-window", "/home/me/a b.md", "/tmp/it's.txt"]]
        );
        assert_eq!(
            argv("firefox %u", &files),
            [
                ["firefox", "file:///home/me/a%20b.md"],
                ["firefox", "file:///tmp/it%27s.txt"]
            ]
        );
        assert_eq!(argv("foot", &files), [["foot"]]);
        assert_eq!(argv("gimp %U %i", &[]), [["gimp"]]);
        assert_eq!(
            argv("app --file=%f", &files[..1]),
            [["app", "--file=/home/me/a b.md"]]
        );
    }

    #[test]
    fn a_files_name_is_never_read_by_a_shell() {
        let evil = [PathBuf::from("/home/me/$(touch pwned)`id`;rm -rf ~.txt")];

        // a code inside a quoted argument isn't filled in, so the name can't land in an
        // sh -c script
        assert_eq!(
            argv(r#"sh -c "viewer %f""#, &evil),
            [["sh", "-c", "viewer %f"]]
        );

        // as its own argument it stays exactly the name
        assert_eq!(
            argv("viewer %f", &evil),
            [["viewer", "/home/me/$(touch pwned)`id`;rm -rf ~.txt"]]
        );
    }

    #[test]
    fn exec_lines_split_as_the_spec_says() {
        assert_eq!(
            argv(r#"env "A=b c" app "it\"s" "\$HOME""#, &[]),
            [["env", "A=b c", "app", "it\"s", "$HOME"]]
        );
        assert_eq!(fill_field_codes(r#"app "open"#, &[]), None);
        assert_eq!(argv("app   two  spaces", &[]), [["app", "two", "spaces"]]);
    }

    // a small seeded generator, so a failure is the same every run
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[test]
    fn any_exec_line_and_file_names_fill_without_panicking() {
        let letters = [
            'a', ' ', '"', '\\', '%', 'f', 'F', 'u', 'U', 'i', '$', '`', '\t', 'é',
        ];
        let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
        for _ in 0..20_000 {
            let exec: String = (0..noise.below(24))
                .map(|_| letters[noise.below(letters.len() as u64) as usize])
                .collect();
            let files: Vec<PathBuf> = (0..noise.below(3))
                .map(|_| {
                    let name: String = (0..noise.below(8))
                        .map(|_| letters[noise.below(letters.len() as u64) as usize])
                        .collect();
                    PathBuf::from(format!("/home/me/{name}"))
                })
                .collect();
            // never a panic; and with a file and a lone %F, each file is an argument as it is
            if let Some(commands) = fill_field_codes(&exec, &files)
                && exec.trim() == "a %F"
            {
                let names: Vec<String> = files
                    .iter()
                    .map(|file| file.to_string_lossy().into_owned())
                    .collect();
                assert_eq!(commands[0][1..], names[..]);
            }
        }
    }

    #[test]
    fn drops_field_codes_but_keeps_double_percent() {
        assert_eq!(strip_field_codes("firefox %u"), "firefox");
        assert_eq!(
            strip_field_codes("code --new-window %F"),
            "code --new-window"
        );
        assert_eq!(strip_field_codes("printf 100%%"), "printf 100%");
    }
}
