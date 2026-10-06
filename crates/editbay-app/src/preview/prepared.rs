use serde::Serialize;
use std::collections::VecDeque;
use uuid::Uuid;

const MAXIMUM_QUEUED: usize = 8;
const MAXIMUM_PINNED_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Reservation {
    session: Uuid,
    pub frame: u64,
}

#[derive(Serialize)]
pub(super) struct State {
    pub session: Uuid,
    pub first: u64,
    pub end: u64,
    pub capacity: usize,
    pub queued: usize,
    pub in_flight: bool,
    pub prepared: u64,
    pub discarded: u64,
    pub open: bool,
    pub maximum_picture_bytes: usize,
    pub maximum_pinned_bytes: usize,
}

pub(super) struct Prepared<T> {
    session: Uuid,
    first: u64,
    end: u64,
    next: u64,
    in_flight: Option<Reservation>,
    capacity: usize,
    maximum_picture_bytes: usize,
    pictures: VecDeque<(u64, T)>,
    prepared: u64,
    discarded: u64,
    open: bool,
}

impl<T> Prepared<T> {
    /// Bound prepared pictures and their retained texture bytes before playback.
    /// `first..end` is the exact range; dimensions bound each FP32 RGBA texture.
    /// Returns a private sequential queue, reserving one additional displayed pin.
    pub(super) fn new(first: u64, end: u64, width: u32, height: u32) -> Result<Self, String> {
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|v| v.checked_mul(16))
            .filter(|&v| v > 0 && v <= MAXIMUM_PINNED_BYTES / 2)
            .ok_or("Picture dimensions exceed the prepared playback budget")?;
        if first >= end {
            return Err("Prepared playback needs a nonempty frame range".into());
        }
        let capacity = MAXIMUM_QUEUED.min(MAXIMUM_PINNED_BYTES / bytes - 1);
        Ok(Self {
            session: Uuid::new_v4(),
            first,
            end,
            next: first,
            in_flight: None,
            capacity,
            maximum_picture_bytes: bytes,
            pictures: VecDeque::with_capacity(capacity),
            prepared: 0,
            discarded: 0,
            open: true,
        })
    }

    /// Reserve the next exact frame without exceeding the combined queue budget.
    /// Takes this queue. Returns no work while full, cancelled, finished or busy.
    pub(super) fn reserve(&mut self) -> Option<Reservation> {
        if !self.open
            || self.in_flight.is_some()
            || self.next == self.end
            || self.pictures.len() == self.capacity
        {
            return None;
        }
        let reservation = Reservation {
            session: self.session,
            frame: self.next,
        };
        self.next += 1;
        self.in_flight = Some(reservation);
        Some(reservation)
    }

    /// Publish one prepared texture under its private reservation.
    /// `reservation` owns `picture`. Returns false and drops obsolete/foreign pins.
    pub(super) fn publish(&mut self, reservation: Reservation, picture: T) -> bool {
        if !self.open || self.in_flight != Some(reservation) {
            return false;
        }
        self.in_flight = None;
        self.pictures.push_back((reservation.frame, picture));
        self.prepared += 1;
        true
    }

    /// Select exactly the sound-clock frame and release already expired pictures.
    /// `frame` is the current exact clock position. Returns only that frame, never
    /// a future picture or an approximation; an absent result exposes underflow.
    pub(super) fn take(&mut self, frame: u64) -> Option<T> {
        if !self.open || frame < self.first || frame >= self.end {
            return None;
        }
        while self.pictures.front().is_some_and(|(at, _)| *at < frame) {
            self.pictures.pop_front();
            self.discarded += 1;
        }
        if self.pictures.front().is_some_and(|(at, _)| *at == frame) {
            self.pictures.pop_front().map(|(_, picture)| picture)
        } else {
            None
        }
    }

    /// Check the initial buffer after the first picture has been selected.
    /// Takes this queue. Returns true only when its bounded future range is ready.
    pub(super) fn ready(&self) -> bool {
        self.open
            && self.pictures.len() as u64 >= (self.end - self.first - 1).min(self.capacity as u64)
    }

    /// Retire this run and release every queued texture pin immediately.
    /// Takes this queue. Returns nothing; an in-flight reservation cannot republish.
    pub(super) fn cancel(&mut self) {
        self.open = false;
        self.pictures.clear();
        self.in_flight = None;
    }

    /// Inspect fixed limits and actual queue occupancy for native evidence.
    /// Takes this queue. Returns counts; submitted GPU draws remain separately
    /// charged by the shared graph's live texture budget until completion.
    pub(super) fn state(&self) -> State {
        State {
            session: self.session,
            first: self.first,
            end: self.end,
            capacity: self.capacity,
            queued: self.pictures.len(),
            in_flight: self.in_flight.is_some(),
            prepared: self.prepared,
            discarded: self.discarded,
            open: self.open,
            maximum_picture_bytes: self.maximum_picture_bytes,
            maximum_pinned_bytes: (self.capacity + 1) * self.maximum_picture_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn dimensions_bound_full_queue_and_an_in_flight_picture() {
        let four_k = Prepared::<()>::new(0, 100, 3840, 2160).unwrap();
        assert_eq!(four_k.capacity, 1);
        assert!(Prepared::<()>::new(0, 100, 7680, 4320).is_err());
        assert!(Prepared::<()>::new(0, 100, 0, 2160).is_err());
        assert!(Prepared::<()>::new(5, 5, 1280, 720).is_err());
        let mut queue = Prepared::new(0, 100, 1920, 1080).unwrap();
        assert_eq!(queue.capacity, 7);
        for _ in 0..7 {
            let reservation = queue.reserve().unwrap();
            assert!(
                queue.pictures.len() + usize::from(queue.in_flight.is_some()) <= queue.capacity
            );
            queue.publish(reservation, ());
        }
        assert!(queue.reserve().is_none());
        assert!(queue.state().maximum_pinned_bytes <= MAXIMUM_PINNED_BYTES);
    }

    #[test]
    fn exact_clock_selection_releases_expired_pins_and_never_shows_future() {
        let mut queue = Prepared::new(10, 20, 1280, 720).unwrap();
        let pin = Arc::new(());
        for _ in 0..queue.capacity {
            let job = queue.reserve().unwrap();
            assert!(queue.reserve().is_none());
            assert!(queue.publish(job, pin.clone()));
        }
        assert!(queue.reserve().is_none());
        assert!(queue.take(9).is_none());
        assert!(queue.take(20).is_none());
        let selected = queue.take(12).unwrap();
        assert_eq!(queue.state().discarded, 2);
        assert_eq!(Arc::strong_count(&pin), queue.pictures.len() + 2);
        drop(selected);
        let late = queue.reserve().unwrap();
        let mut other = Prepared::<Arc<()>>::new(10, 20, 1280, 720).unwrap();
        let own = other.reserve().unwrap();
        assert!(!other.publish(late, pin.clone()));
        assert!(other.publish(own, pin.clone()));
        other.cancel();
        queue.cancel();
        assert!(!queue.publish(late, pin.clone()));
        assert!(queue.reserve().is_none());
        assert!(queue.take(13).is_none());
        assert_eq!(Arc::strong_count(&pin), 1);
    }

    #[test]
    fn one_frame_and_partial_tails_require_every_available_initial_picture() {
        for count in [1, 2, 4, 12] {
            let mut queue = Prepared::new(0, count, 1280, 720).unwrap();
            let first = queue.reserve().unwrap();
            queue.publish(first, ());
            queue.take(0).unwrap();
            for _ in 0..(count - 1).min(queue.capacity as u64) {
                assert!(!queue.ready());
                let next = queue.reserve().unwrap();
                queue.publish(next, ());
            }
            assert!(queue.ready());
            assert!(queue.state().maximum_pinned_bytes <= MAXIMUM_PINNED_BYTES);
        }
    }
}
