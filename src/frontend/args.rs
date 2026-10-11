//! The `std/args` frontend library. Inputs are resolved before producing IR;
//! simulation expressions never read the host process's arguments.

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
    Ok(())
}

/// Read `--name value` or `--name=value` after the CLI's `--` separator.
/// A number may be negative or infinite, but never NaN. Repeated options
/// keep command-line order, just as instance and run-setting inputs do.
pub fn numbers(args: &[String]) -> Result<Vec<(String, f64)>, String> {
    let mut values = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let option = arg
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected program argument `{arg}`; expected --name value"))?;
        let (name, value) = match option.split_once('=') {
            Some(pair) => pair,
            None => (
                option,
                args.next()
                    .map(String::as_str)
                    .ok_or_else(|| format!("missing value for program argument `--{option}`"))?,
            ),
        };
        check_name(name)?;
        let number = value
            .parse::<f64>()
            .ok()
            .filter(|v| !v.is_nan())
            .ok_or_else(|| format!("invalid number `{value}` for program argument `--{name}`"))?;
        values.push((name.to_string(), number));
    }
    Ok(values)
}
