//! The `std/args` frontend library. Inputs are resolved before producing IR;
//! simulation expressions never read the host process's arguments.

/// The interpreter's own command-line options, `--seed` and the rest. A
/// program's inputs share the command line with them (`--seed 2 --Lambda
/// 0.3`), so an input may not take one of these names.
pub const OPTIONS: [&str; 14] = [
    "workload",
    "seed",
    "horizon",
    "warmup",
    "arrivals",
    "trace",
    "instance",
    "def",
    "json",
    "dump",
    "log",
    "inline-trace",
    "format",
    "out",
];

/// Argument names are identifiers, shared by CLI, instance and API inputs.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name.chars().enumerate().all(|(i, c)| {
            c == '_'
                || if i == 0 {
                    c.is_alphabetic()
                } else {
                    c.is_alphanumeric()
                }
        })
    {
        return Err(format!(
            "invalid argument name `{name}`; expected an identifier"
        ));
    }
    if OPTIONS.contains(&name) {
        return Err(format!(
            "argument name `{name}` is the interpreter's option `--{name}`\nhelp: name the input otherwise, for example `{name}_`"
        ));
    }
    Ok(())
}

/// Read one program argument, `--name value` or `--name=value`, whose
/// `option` is the text after `--` and `value` the next word if the option
/// has no `=`. A number may be negative or infinite, but never NaN.
/// Returns the name, the number, and whether `value` was consumed.
pub fn number(option: &str, value: Option<&str>) -> Result<(String, f64, bool), String> {
    let (name, value, consumed) = match option.split_once('=') {
        Some((name, value)) => (name, value, false),
        None => (
            option,
            value.ok_or_else(|| format!("missing value for program argument `--{option}`\nhelp: write --name value, for example --Lambda 0.3"))?,
            true,
        ),
    };
    check_name(name).map_err(|e| {
        if e.contains("help:") {
            e
        } else {
            format!("{e}\nhelp: write --name value, for example --Lambda 0.3")
        }
    })?;
    let number = value
        .parse::<f64>()
        .ok()
        .filter(|v| !v.is_nan())
        .ok_or_else(|| {
            format!("invalid number `{value}` for program argument `--{name}`\nhelp: write --name value, for example --Lambda 0.3")
        })?;
    Ok((name.to_string(), number, consumed))
}
