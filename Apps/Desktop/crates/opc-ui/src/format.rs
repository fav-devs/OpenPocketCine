//! Stepping through the video formats the body says it has.
//!
//! Resolution and frame rate are not independent — a body that shoots 4K may only offer
//! 24, 25 and 30 there while 1080p goes to 120 — so neither can be chosen alone. Both
//! keys walk the camera's own list of pairs, which means an operator can never ask for a
//! combination the body does not have.
//!
//! The list arrives from the camera in its own order and is used in that order. Sorting
//! it here would be this shell inventing a ladder the body did not describe.

/// The next resolution after `current`, keeping the frame rate if that pair exists.
///
/// Returns `None` when there is nothing to move to: an empty list, or one resolution.
pub fn next_resolution(available: &[(u8, u8)], current: Option<(u8, u8)>) -> Option<(u8, u8)> {
    let mut resolutions: Vec<u8> = Vec::new();
    for (resolution, _) in available {
        if !resolutions.contains(resolution) {
            resolutions.push(*resolution);
        }
    }
    if resolutions.len() < 2 {
        return None;
    }
    let (current_resolution, current_rate) = current?;
    let at = resolutions.iter().position(|r| *r == current_resolution)?;
    let wanted = resolutions[(at + 1) % resolutions.len()];
    // Keep the frame rate across the step when the body shoots it at both.
    if available.contains(&(wanted, current_rate)) {
        return Some((wanted, current_rate));
    }
    available
        .iter()
        .find(|(resolution, _)| *resolution == wanted)
        .copied()
}

/// The next frame rate at the current resolution.
///
/// Returns `None` when that resolution has only one, so a key press that cannot change
/// anything sends nothing rather than a command the camera would echo back unchanged.
pub fn next_frame_rate(available: &[(u8, u8)], current: Option<(u8, u8)>) -> Option<(u8, u8)> {
    let (current_resolution, current_rate) = current?;
    let rates: Vec<u8> = available
        .iter()
        .filter(|(resolution, _)| *resolution == current_resolution)
        .map(|(_, rate)| *rate)
        .collect();
    if rates.len() < 2 {
        return None;
    }
    let at = rates.iter().position(|rate| *rate == current_rate)?;
    Some((current_resolution, rates[(at + 1) % rates.len()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two resolutions, and only the smaller one shoots fast.
    const LADDER: [(u8, u8); 5] = [(1, 24), (1, 30), (1, 60), (2, 24), (2, 30)];

    #[test]
    fn the_frame_rate_walks_the_rates_at_this_resolution_and_wraps() {
        assert_eq!(next_frame_rate(&LADDER, Some((1, 24))), Some((1, 30)));
        assert_eq!(next_frame_rate(&LADDER, Some((1, 30))), Some((1, 60)));
        assert_eq!(next_frame_rate(&LADDER, Some((1, 60))), Some((1, 24)));
        assert_eq!(next_frame_rate(&LADDER, Some((2, 30))), Some((2, 24)));
    }

    #[test]
    fn a_rate_the_other_resolution_does_not_have_is_never_offered() {
        // 60 exists at resolution 1 only, so stepping resolution 2 must not reach it.
        for _ in 0..4 {
            let stepped = next_frame_rate(&LADDER, Some((2, 24))).expect("two rates at 2");
            assert!(
                LADDER.contains(&stepped),
                "{stepped:?} is not on the ladder"
            );
            assert_ne!(stepped.1, 60);
        }
    }

    #[test]
    fn the_resolution_keeps_the_frame_rate_when_both_shoot_it() {
        assert_eq!(next_resolution(&LADDER, Some((1, 30))), Some((2, 30)));
        assert_eq!(next_resolution(&LADDER, Some((2, 24))), Some((1, 24)));
    }

    #[test]
    fn a_frame_rate_the_next_resolution_cannot_do_falls_to_its_first() {
        // 60 does not exist at resolution 2, so the step lands on what does.
        assert_eq!(next_resolution(&LADDER, Some((1, 60))), Some((2, 24)));
    }

    #[test]
    fn nothing_to_step_to_is_nothing_sent() {
        assert_eq!(next_resolution(&[], Some((1, 30))), None);
        assert_eq!(next_resolution(&[(1, 30), (1, 60)], Some((1, 30))), None);
        assert_eq!(next_frame_rate(&[(1, 30)], Some((1, 30))), None);
        assert_eq!(next_frame_rate(&LADDER, None), None);
        assert_eq!(next_resolution(&LADDER, None), None);
    }

    #[test]
    fn a_format_the_body_did_not_list_steps_nowhere() {
        // The camera is in a mode this build has not seen. Guessing at a neighbour on a
        // ladder it is not on would change the shot to something unasked for.
        assert_eq!(next_frame_rate(&LADDER, Some((9, 30))), None);
        assert_eq!(next_resolution(&LADDER, Some((9, 30))), None);
        assert_eq!(next_frame_rate(&LADDER, Some((1, 99))), None);
    }
}
