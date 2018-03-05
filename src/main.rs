extern crate clap;
extern crate tempfile;
extern crate os_pipe;
extern crate scopeguard;

use std::io::{self, Write};
use std::fs;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use clap::{App, Arg, ArgGroup, AppSettings};


#[derive(Clone)]
enum StdIn<'a> {
    Null,
    Redirect(&'a OsStr),
    Inherit,
}


#[derive(Clone)]
enum StdStream<'a> {
    Null,
    Redirect(&'a OsStr),
    Inherit,
    Other,
}


fn stream_file_arg<'a>(arg: Option<&'a OsStr>) -> StdStream<'a> {
    if let Some(s) = arg {
        if s == OsStr::new("-") {
            return StdStream::Inherit;
        }
        return StdStream::Redirect(s);
    }
    StdStream::Inherit
}


fn stdin_file_arg<'a>(arg: Option<&'a OsStr>) -> StdIn<'a> {
    if let Some(s) = arg {
        if s == OsStr::new("-") {
            return StdIn::Inherit;
        }
        return StdIn::Redirect(s);
    }
    StdIn::Inherit
}



fn create_stdio<F>(stdin: &StdIn,
                   stdout: &StdStream,
                   stderr: &StdStream,
                   mut create_file: F) -> (std::process::Stdio,
                                           std::process::Stdio,
                                           std::process::Stdio)
    where F: FnMut(&Path) -> io::Result<fs::File>
{
    use std::process::Stdio;

    let stdio_in = match *stdin {
        StdIn::Null => Stdio::null(),
        StdIn::Inherit => Stdio::inherit(),
        StdIn::Redirect(path) => {
            fs::File::open(path).expect("Failed to open STDIN file!").into()
        }
    };

    let (stdio_out, stdio_err): (Stdio, Stdio) = match (stdout, stderr) {
        (&StdStream::Other, &StdStream::Other) => {
            let e = os_pipe::parent_stdout().expect("Failed to DUP STDERR!");
            let o = os_pipe::parent_stderr().expect("Failed to DUP STDOUT!");
            (o, e)
        },
        (&StdStream::Other, &StdStream::Redirect(path)) |
        (&StdStream::Redirect(path), &StdStream::Other) => {
            let f = create_file(path.as_ref())
                .expect("Failed to create STDOUT/ERR file!");
            (f.try_clone().expect("Failed to DUP STDOUT/ERR file!").into(),
             f.into())
        },
        (_stdout, _stderr) => {
            let out = match _stdout {
                &StdStream::Null => Stdio::null(),
                &StdStream::Inherit => Stdio::inherit(),
                &StdStream::Redirect(path) => {
                    create_file(path.as_ref())
                        .expect("Failed to create STDOUT file!").into()
                }
                _ => unreachable!()
            };
            let err = match _stderr {
                &StdStream::Null => Stdio::null(),
                &StdStream::Inherit => Stdio::inherit(),
                &StdStream::Redirect(path) => {
                    create_file(path.as_ref())
                        .expect("Failed to create STDERR file!").into()
                }
                _ => unreachable!()
            };
            (out, err)
        }
    };

    (stdio_in, stdio_out, stdio_err)
}



fn env_to_kv<'a>(arg: &'a OsStr) -> (&'a OsStr, &'a OsStr) {
    let data = arg.to_str().map(|s| s.as_bytes()).unwrap();
    for (i, b) in data.iter().enumerate() {
        if *b == ('=' as u8) {
            unsafe {
                return (
                    std::mem::transmute(&data[..i]),
                    std::mem::transmute(&data[i + 1..])
                )
            }
        }
    }
    (arg, unsafe { std::mem::transmute(&data[data.len() .. data.len()]) })
}


struct Argv<'a> {
    stdin: StdIn<'a>,
    stdout: StdStream<'a>,
    stderr: StdStream<'a>,
    command: clap::OsValues<'a>,
    workdir: Option<&'a OsStr>,
    env_unset: Option<clap::OsValues<'a>>,
    env_set: Option<clap::OsValues<'a>>,
    env_clear: bool,
    tmpdir: Option<&'a OsStr>,
    exitfile: Option<&'a OsStr>,
    is_atomic: bool,
}


fn configure_args<'a, 'b>() -> App<'a, 'b> {
    App::new("Subprocess environment handler")
        .setting(AppSettings::TrailingVarArg)
        .setting(AppSettings::DontDelimitTrailingValues)
        .version("1.0")
        .author("Andrey Bushev")

        .arg(Arg::with_name("noenv")
             .short("n")
             .long("no-environment"))
        .arg(Arg::with_name("unset")
             .short("u")
             .long("unset")
             .takes_value(true))
        .arg(Arg::with_name("workdir")
             .short("w")
             .long("workdir")
             .takes_value(true))
        .arg(Arg::with_name("atomic")
             .long("atomic"))
        .arg(Arg::with_name("tmpdir")
             .long("tmpdir")
             .takes_value(true))
        .arg(Arg::with_name("exitfile")
             .short("f")
             .long("exit-file")
             .takes_value(true))
        .arg(Arg::with_name("env")
             .short("e")
             .long("env")
             .takes_value(true)
             .number_of_values(1)
             .use_delimiter(false))

        // STDIN
        .arg(Arg::with_name("in_file")
             .long("--in-file")
             .takes_value(true))
        .arg(Arg::with_name("in_null")
             .long("--in-null"))
        .group(ArgGroup::with_name("STDIN")
               .args(&["in_file", "in_null"]))

        // STDOUT
        .arg(Arg::with_name("out_file")
             .long("--out-file")
             .takes_value(true))
        .arg(Arg::with_name("out_null")
             .long("--out-null"))
        .arg(Arg::with_name("out_err")
             .long("--out-err"))
        .group(ArgGroup::with_name("STDOUT")
               .args(&["out_file", "out_null", "out_err"]))

        // STDERR
        .arg(Arg::with_name("err_file")
             .long("--err-file")
             .takes_value(true))
        .arg(Arg::with_name("err_null")
             .long("--err-null"))
        .arg(Arg::with_name("err_out")
             .long("--err-out"))
        .group(ArgGroup::with_name("STDERR")
               .args(&["err_file","err_null", "err_out"]))

        .arg(Arg::with_name("command")
             .help("trailing arguments to execute")
             .multiple(true)
             .required(true))
}


fn extract_args<'a>(argv: &'a clap::ArgMatches<'a>) -> Argv<'a> {

    let stdin_arg = if argv.is_present("in_null") {
        StdIn::Null
    } else {
        stdin_file_arg(argv.value_of_os("in_file"))
    };

    let stdout_arg = if argv.is_present("out_null") {
        StdStream::Null
    } else if argv.is_present("out_err") {
        StdStream::Other
    } else {
        stream_file_arg(argv.value_of_os("out_file"))
    };

    let stderr_arg = if argv.is_present("err_null") {
        StdStream::Null
    } else if argv.is_present("err_out") {
        StdStream::Other
    } else {
        stream_file_arg(argv.value_of_os("err_file"))
    };

    return Argv {
        stdin: stdin_arg,
        stdout: stdout_arg,
        stderr: stderr_arg,
        command: argv.values_of_os("command").unwrap(),
        workdir: argv.value_of_os("workdir"),
        env_unset: argv.values_of_os("unset"),
        env_set: argv.values_of_os("env"),
        env_clear: argv.is_present("noenv"),
        is_atomic: argv.is_present("atomic"),
        tmpdir: argv.value_of_os("tmpdir"),
        exitfile: argv.value_of_os("exitfile")
    };
}



fn run() -> std::process::ExitStatus {
    let matches = configure_args().get_matches();
    let mut argv = extract_args(&matches);

    let mut _to_move: Vec<(tempfile::NamedTempFile, PathBuf)> = Vec::new();
    let mut to_move = scopeguard::guard(_to_move, |m| {
        for (file, dest) in m.drain(..) {
            drop(file.persist(dest));
        }
    });

    let name = argv.command.next().expect("Command is empty!");

    let tempdir = if let Some(path) = argv.tmpdir {
        fs::create_dir_all(path).expect("Failed to create TMPDIR!");
        PathBuf::from(path)
    } else {
        std::env::temp_dir()
    };

    let mut child = std::process::Command::new(name);
    child.args(argv.command);

    let (stdin, stdout, stderr) = if argv.is_atomic {
        create_stdio(&argv.stdin, &argv.stdout, &argv.stderr,
                     |path| {
                         let tmp = tempfile::NamedTempFileOptions::new()
                             .create_in(&tempdir)?;
                         let f = tmp.reopen()?;
                         to_move.push((tmp, path.into()));
                         Ok(f)
                     })
    } else {
        create_stdio(&argv.stdin, &argv.stdout, &argv.stderr,
                     |path| fs::File::create(path))
    };

    child.stdin(stdin);
    child.stdout(stdout);
    child.stderr(stderr);

    if let Some(wd) = argv.workdir {
        fs::create_dir_all(wd).expect("Failed to create working dir!");
        child.current_dir(wd);
    }

    if argv.env_clear {
        child.env_clear();
    }

    if let Some(unset) = argv.env_unset {
        for n in unset {
            child.env_remove(n);
        }
    }

    if let Some(envs) = argv.env_set {
        for (k, v) in envs.map(env_to_kv) {
            child.env(k, v);
        }
    }

    let mut result = child.spawn()
        .expect("Failed to run command!");

    let exitcode = result.wait().unwrap();

    if let Some(path) = argv.exitfile {
        if argv.is_atomic {
            let mut tmp = tempfile::NamedTempFileOptions::new()
                .create_in(tempdir)
                .expect("Failed to create TMP exitcode file!");
            write!(tmp, "{}", exitcode.code().unwrap_or(0))
                .expect("Failed to write to TMP exitcode file!");
            tmp.persist(path)
                .expect("Failed to move TMP exitcode file!");
        } else {
            let mut f = fs::File::create(path)
                .expect("Failed to create exitcode file");
            write!(f, "{}", exitcode.code().unwrap_or(0))
                .expect("Failed to write to exitcode file!");
        }
    }

    return exitcode;
}



fn main() {
    std::process::exit(run().code().unwrap_or(0));
}
