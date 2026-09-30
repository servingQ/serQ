//! Replayed sessions: the `session,turn,new,out,think[,forced]` CSV a
//! program's `trace` names (`examples/replay/data/*.csv`).

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    pub new: f64,
    pub out: f64,
    pub think: f64,
    /// Optional sixth column: 1 if the turn is forced to miss (a nonce at
    /// the head of its prompt), 0 otherwise.
    pub forced: f64,
}

#[derive(Clone, Debug, Default)]
pub struct TraceSession {
    pub turns: Vec<Turn>,
}

#[derive(Clone, Debug, Default)]
pub struct Corpus {
    pub sessions: Vec<TraceSession>,
}

impl Corpus {
    pub fn from_csv(text: &str) -> Result<Self, String> {
        let mut sessions: Vec<TraceSession> = vec![];
        let mut last: Option<u64> = None;
        for (ln, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("session") {
                continue;
            }
            let f: Vec<&str> = line.split(',').collect();
            if f.len() != 5 && f.len() != 6 {
                return Err(format!(
                    "line {}: expected 5 or 6 fields, got {}\nhelp: use session,turn,new,out,think[,forced]",
                    ln + 1,
                    f.len()
                ));
            }
            let num = |i: usize| -> Result<f64, String> {
                const COLUMNS: [&str; 6] = ["session", "turn", "new", "out", "think", "forced"];
                f[i].parse::<f64>().map_err(|_| format!(
                    "line {}: column {} ({}): invalid number `{}`\nhelp: replace `{}` with a numeric value",
                    ln + 1, i + 1, COLUMNS[i], f[i], COLUMNS[i]
                ))
            };
            let sid = num(0)? as u64;
            if last != Some(sid) {
                sessions.push(TraceSession::default());
                last = Some(sid);
            }
            sessions.last_mut().unwrap().turns.push(Turn {
                new: num(2)?,
                out: num(3)?,
                think: num(4)?,
                forced: if f.len() == 6 { num(5)? } else { 0.0 },
            });
        }
        if sessions.is_empty() {
            return Err(
                "empty trace\nhelp: add a data row using session,turn,new,out,think[,forced]"
                    .into(),
            );
        }
        Ok(Corpus { sessions })
    }

    pub fn turns(&self) -> usize {
        self.sessions.iter().map(|s| s.turns.len()).sum()
    }
}
