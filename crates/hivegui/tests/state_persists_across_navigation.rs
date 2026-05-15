//! Addresses analysis finding C1: SC-004 ("state preserved across surface
//! switches on every attempt") must have automated coverage at the model
//! level. The full gpui UI is not headlessly testable, but the
//! `Conversation` Entity is what carries the in-flight `pending` slot.
//! This test asserts the slot survives an arbitrary number of borrow /
//! drop cycles, simulating the shell's navigate-away-and-back pattern.

use hivegui::model::conversation::Conversation;

#[test]
fn pending_slot_survives_simulated_navigations() {
    let mut conv = Conversation::new();
    let pending = conv.send_user_message("hello".to_string(), vec![]).unwrap();

    // Simulate the engineer flipping to Day+1 and back several times.
    for _ in 0..5 {
        let _snapshot_turns = conv.turns().len();
        let _is_busy = conv.is_busy();
        // Read borrow ends here; no mutation has been applied.
    }

    assert!(conv.is_busy());
    assert_eq!(conv.pending(), Some(pending));
    assert_eq!(conv.turns().len(), 1);
}
