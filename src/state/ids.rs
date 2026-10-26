use jiff::Timestamp;
use rand::Rng;

/// Base32 digits in ASCII order, so comparing ids as strings compares their timestamps.
const ALPHABET: &[u8; 32] = b"234567abcdefghijklmnopqrstuvwxyz";
const TIME_CHARS: u32 = 6;
const RANDOM_CHARS: usize = 2;
const ID_LEN: usize = TIME_CHARS as usize + RANDOM_CHARS;
const MAX_SECONDS: i64 = (1 << (5 * TIME_CHARS)) - 1;

/// Seconds since 2026-01-01T00:00:00Z.
const ID_EPOCH_SECONDS: i64 = 1_767_225_600;

/// Six base32 characters of seconds since the id epoch (big-endian), then two random ones. The
/// alphabet is in ASCII order, so ids sort as strings by creation second.
pub fn new_task_id(now: Timestamp, rng: &mut impl Rng) -> String {
    let seconds = (now.as_second() - ID_EPOCH_SECONDS).clamp(0, MAX_SECONDS);
    let time = (0..TIME_CHARS).rev().map(|i| {
        let digit = (seconds >> (5 * i)) & 0x1f;
        ALPHABET[digit as usize]
    });
    let random = (0..RANDOM_CHARS).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())]);
    time.chain(random).map(char::from).collect()
}

/// Whether `s` has the shape of a task id. Keeps ids from naming paths outside `state/tasks`.
pub fn is_task_id(s: &str) -> bool {
    s.len() == ID_LEN && s.bytes().all(|b| ALPHABET.contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn ids_are_8_base32_chars() {
        let mut rng = StdRng::seed_from_u64(7);
        let now: Timestamp = "2026-10-26T12:00:00Z".parse().unwrap();
        for _ in 0..100 {
            let id = new_task_id(now, &mut rng);
            assert_eq!(id.len(), 8, "{id}");
            assert!(id.bytes().all(|b| ALPHABET.contains(&b)), "{id}");
        }
    }

    #[test]
    fn ids_sort_by_time() {
        let mut rng = StdRng::seed_from_u64(7);
        // 12:01:03Z is 1023 mod 1024 seconds past the id epoch, so the next second rolls the last
        // two digits over from `z` to `2`, and the window crosses two more last-digit rollovers.
        let start: Timestamp = "2026-10-26T12:01:03Z".parse().unwrap();
        let ids: Vec<_> = (0..64)
            .map(|s| new_task_id(start + jiff::SignedDuration::from_secs(s), &mut rng))
            .collect();
        for pair in ids.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{} should sort before {}",
                pair[0],
                pair[1]
            );
        }
    }
}
