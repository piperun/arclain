use super::TokenBucket;
use std::time::Duration;

#[test]
fn token_bucket_allows_burst_up_to_capacity() {
    let bucket = TokenBucket::new(1000.0, 5000); // 1000 tokens/sec, capacity 5000
    // Start with full capacity — burst of 5000 should all pass
    for _ in 0..5000 {
        assert!(bucket.try_take(), "expected token within capacity");
    }
    // 5001st request in the same instant — refused
    assert!(!bucket.try_take(), "expected refusal beyond capacity");
}

#[test]
fn token_bucket_refills_at_configured_rate() {
    let bucket = TokenBucket::new(1000.0, 100); // 1000/sec, cap 100
    // Drain it
    for _ in 0..100 {
        bucket.try_take();
    }
    assert!(!bucket.try_take(), "drained");
    // Wait 50 ms — should refill ~50 tokens
    std::thread::sleep(Duration::from_millis(50));
    let mut taken = 0;
    while bucket.try_take() {
        taken += 1;
        if taken > 100 {
            break;
        }
    }
    // Allow some slack for scheduling jitter
    assert!(
        (40..=60).contains(&taken),
        "expected ~50 refilled tokens after 50 ms, got {}",
        taken
    );
}
