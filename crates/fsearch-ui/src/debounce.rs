use std::time::{Duration, Instant};

pub struct Debouncer {
    delay: Duration,
    last_input: Instant,
    pending: bool,
    primed: bool,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            last_input: Instant::now(),
            pending: false,
            primed: false,
        }
    }

    pub fn on_input(&mut self, now: Instant) -> bool {
        self.last_input = now;
        if !self.primed {
            self.primed = true;
            self.pending = false;
            return true;
        }
        self.pending = true;
        false
    }

    pub fn should_fire(&mut self, now: Instant) -> bool {
        if !self.pending {
            return false;
        }
        if now.duration_since(self.last_input) >= self.delay {
            self.pending = false;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_input_fires_immediately() {
        let mut d = Debouncer::new(Duration::from_millis(30));
        assert!(d.on_input(Instant::now()));
    }

    #[test]
    fn rapid_input_waits_for_quiet_period() {
        let mut d = Debouncer::new(Duration::from_millis(30));
        let t0 = Instant::now();
        d.on_input(t0);
        assert!(!d.on_input(t0 + Duration::from_millis(10)));
        assert!(!d.should_fire(t0 + Duration::from_millis(25)));
        assert!(d.should_fire(t0 + Duration::from_millis(35)));
    }

    #[test]
    fn after_firing_quiet_period_resets() {
        let mut d = Debouncer::new(Duration::from_millis(30));
        let t0 = Instant::now();
        d.on_input(t0);
        assert!(d.should_fire(t0 + Duration::from_millis(40)));
        assert!(!d.should_fire(t0 + Duration::from_millis(45)));
        d.on_input(t0 + Duration::from_millis(50));
        assert!(d.should_fire(t0 + Duration::from_millis(85)));
    }
}
