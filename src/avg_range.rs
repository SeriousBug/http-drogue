pub struct MovingAverage {
    window_bytes: [u64; 60],
    window_time: [u64; 60],
    index: usize,
    high_water: usize,
}

impl MovingAverage {
    pub fn new() -> Self {
        Self {
            window_bytes: [0; 60],
            window_time: [0; 60],
            index: 0,
            high_water: 0,
        }
    }

    pub fn add(&mut self, bytes: u64, time: u64) {
        self.window_bytes[self.index] = bytes;
        self.window_time[self.index] = time;
        self.index = (self.index + 1) % 60;
        if self.high_water < 60 {
            self.high_water += 1;
        }
    }

    pub fn average(&self) -> f64 {
        let mut sum_bytes = 0u64;
        let mut sum_time = 0u64;
        for i in 0..self.high_water {
            sum_bytes += self.window_bytes[i];
            sum_time += self.window_time[i];
        }
        (sum_bytes as f64) / (sum_time as f64)
    }
}
