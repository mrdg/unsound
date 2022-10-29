use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

#[derive(Debug, PartialEq, PartialOrd, Clone, Copy)]
pub struct Frame<const N: usize>([f32; N]);

impl<const N: usize> Frame<N> {
    pub const ZERO: Frame<N> = Frame([0.0; N]);

    pub fn new(samples: [f32; N]) -> Frame<N> {
        Self(samples)
    }

    pub fn channel(&self, index: usize) -> f32 {
        self.0[index]
    }

    fn channel_mut(&mut self, index: usize) -> &mut f32 {
        &mut self.0[index]
    }

    pub fn map<F>(&self, mut f: F) -> Frame<N>
    where
        F: FnMut(f32) -> f32,
    {
        let mut output = Self::ZERO;
        for ch in 0..N {
            *output.channel_mut(ch) = f(self.channel(ch))
        }
        output
    }

    pub fn zip<F>(&self, other: Frame<N>, mut f: F) -> Frame<N>
    where
        F: FnMut(f32, f32) -> f32,
    {
        let mut output = Self::ZERO;
        for ch in 0..N {
            *output.channel_mut(ch) = f(self.channel(ch), other.channel(ch));
        }
        output
    }
}

impl<const N: usize> Add for Frame<N> {
    type Output = Frame<N>;

    fn add(self, other: Frame<N>) -> Self::Output {
        self.zip(other, |a, b| a + b)
    }
}

impl<const N: usize> AddAssign for Frame<N> {
    fn add_assign(&mut self, other: Frame<N>) {
        *self = *self + other;
    }
}

impl<const N: usize> Sub for Frame<N> {
    type Output = Frame<N>;

    fn sub(self, other: Frame<N>) -> Self::Output {
        self.zip(other, |a, b| a - b)
    }
}

impl<const N: usize> SubAssign for Frame<N> {
    fn sub_assign(&mut self, other: Frame<N>) {
        *self = *self - other;
    }
}

impl<const N: usize> Mul<Frame<N>> for Frame<N> {
    type Output = Frame<N>;

    fn mul(self, other: Frame<N>) -> Self::Output {
        self.zip(other, |a, b| a * b)
    }
}

impl<const N: usize> Mul<f32> for Frame<N> {
    type Output = Frame<N>;

    fn mul(self, other: f32) -> Self::Output {
        self.map(|sample| sample * other)
    }
}

impl<const N: usize> Div<f32> for Frame<N> {
    type Output = Frame<N>;

    fn div(self, other: f32) -> Self::Output {
        self.map(|sample| sample / other)
    }
}

pub type Stereo = Frame<2>;

pub type Buffer = Vec<Stereo>;

// TODO: consider recalculating the sum every so often to prevent floating point
// inaccuracies over time
pub struct Rms {
    squared: Vec<Stereo>,
    sum: Stereo,
    position: usize,
    window_length: usize,
}

impl Rms {
    pub fn new(window_size: usize) -> Self {
        Self {
            squared: vec![Stereo::ZERO; window_size],
            sum: Stereo::ZERO,
            position: 0,
            window_length: 0,
        }
    }

    pub fn add_frame(&mut self, frame: Stereo) {
        self.sum -= self.squared[self.position];
        let squared = frame * frame;
        self.sum += squared;
        self.squared[self.position] = squared;
        self.position += 1;
        if self.position >= self.squared.len() {
            self.position = 0;
        }
        self.window_length = usize::min(self.window_length + 1, self.squared.len());
    }

    pub fn value(&self) -> Stereo {
        let mean = self.sum / self.window_length as f32;
        mean.map(|sample| sample.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! frame {
        ($elem:expr; $n:expr) => (
            Frame::new([$elem; $n])
        );
        ($($x:expr),+ $(,)?) => (
            Frame::new([$($x),+])
        );
    }

    fn add_frames(rms: &mut Rms, frames: &[Stereo]) {
        for frame in frames {
            rms.add_frame(*frame);
        }
    }

    #[test]
    fn frame_add() {
        let a = frame![0.5, 0.75];
        let b = frame![0.25, 0.25];
        let c = a + b;
        assert_eq!(c, frame![0.75, 1.0]);
    }

    #[test]
    fn frame_add_assign() {
        let mut a = frame![0.5, 0.75];
        let b = frame![0.25, 0.25];
        a += b;
        assert_eq!(a, frame![0.75, 1.0]);
    }

    #[test]
    fn frame_scale() {
        let a = frame![0.5, 0.2];
        let b = a * 0.5;
        assert_eq!(b, frame![0.25, 0.1]);
    }

    #[test]
    fn rms() {
        let mut rms = Rms::new(8);
        add_frames(
            &mut rms,
            &[
                frame![0.5, 0.5],
                frame![-0.5, -0.5],
                frame![0.5, 0.5],
                frame![-0.5, -0.5],
            ],
        );
        assert_eq!(frame![0.5, 0.5], rms.value());
        add_frames(
            &mut rms,
            &[
                frame![0.3, 0.3],
                frame![-0.3, -0.3],
                frame![0.3, 0.3],
                frame![-0.3, -0.3],
                frame![-0.3, -0.3],
            ],
        );
        assert_eq!(frame![0.38729838, 0.38729838], rms.value());
    }
}
