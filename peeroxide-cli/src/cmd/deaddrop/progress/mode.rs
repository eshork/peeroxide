#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressMode {
    Bar,
    PeriodicLog,
    Json,
    Off,
}

pub fn select(stderr_is_tty: bool, no_progress: bool, json: bool) -> ProgressMode {
    if json {
        ProgressMode::Json
    } else if no_progress {
        ProgressMode::Off
    } else if stderr_is_tty {
        ProgressMode::Bar
    } else {
        ProgressMode::PeriodicLog
    }
}

#[cfg(test)]
mod tests {
    use super::{select, ProgressMode};

    #[test]
    fn tty_and_no_progress_off_with_json() {
        assert_eq!(select(false, true, true), ProgressMode::Json);
    }

    #[test]
    fn tty_and_progress_bar() {
        assert_eq!(select(true, false, false), ProgressMode::Bar);
    }

    #[test]
    fn tty_and_no_progress_off() {
        assert_eq!(select(true, true, false), ProgressMode::Off);
    }

    #[test]
    fn tty_and_json_wins() {
        assert_eq!(select(true, false, true), ProgressMode::Json);
    }

    #[test]
    fn non_tty_and_periodic_log() {
        assert_eq!(select(false, false, false), ProgressMode::PeriodicLog);
    }

    #[test]
    fn non_tty_and_no_progress_off() {
        assert_eq!(select(false, true, false), ProgressMode::Off);
    }

    #[test]
    fn non_tty_and_json_wins() {
        assert_eq!(select(false, false, true), ProgressMode::Json);
    }

    #[test]
    fn non_tty_no_progress_json_still_wins() {
        assert_eq!(select(false, true, true), ProgressMode::Json);
    }
}
