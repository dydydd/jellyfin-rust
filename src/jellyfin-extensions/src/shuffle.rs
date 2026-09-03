use rand::Rng;

/// Shuffles a slice in place using the current thread's shared random generator.
pub fn shuffle<T>(values: &mut [T]) {
    shuffle_with(values, &mut rand::rng());
}

/// Shuffles a slice in place using an injected random generator.
///
/// This is the backward Fisher-Yates algorithm used by Jellyfin: at each step,
/// an index is chosen from the still-unshuffled prefix before its length shrinks.
pub fn shuffle_with<T>(values: &mut [T], rng: &mut impl Rng) {
    let mut remaining = values.len();
    while remaining > 1 {
        let selected = rng.random_range(0..remaining);
        remaining -= 1;
        values.swap(selected, remaining);
    }
}
