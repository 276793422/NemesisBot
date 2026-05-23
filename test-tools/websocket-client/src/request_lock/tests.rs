use super::*;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_request_lock_basic() {
    let lock = RequestLock::new();

    // First acquire should succeed
    assert!(lock.try_acquire("test1".to_string()).await.is_ok());

    // Second acquire should fail while first is held
    assert!(lock.try_acquire("test2".to_string()).await.is_err());

    // Release and try again
    lock.release().await;
    assert!(lock.try_acquire("test3".to_string()).await.is_ok());

    lock.release().await;
}

#[tokio::test]
async fn test_request_lock_is_busy() {
    let lock = RequestLock::new();

    // Not busy initially
    assert!(!lock.is_busy().await);

    // Acquire and check
    lock.try_acquire("test".to_string()).await.unwrap();
    assert!(lock.is_busy().await);

    // Release and check
    lock.release().await;
    assert!(!lock.is_busy().await);
}

#[tokio::test]
async fn test_request_lock_concurrent() {
    let lock = Arc::new(RequestLock::new());
    let lock1 = lock.clone();
    let lock2 = lock.clone();

    let task1 = tokio::spawn(async move {
        for i in 1..=3 {
            while lock1.try_acquire(format!("task1-{}", i).to_string()).await.is_err() {
                sleep(Duration::from_millis(10)).await;
            }
            sleep(Duration::from_millis(50)).await;
            lock1.release().await;
        }
    });

    let task2 = tokio::spawn(async move {
        for i in 1..=3 {
            while lock2.try_acquire(format!("task2-{}", i).to_string()).await.is_err() {
                sleep(Duration::from_millis(10)).await;
            }
            sleep(Duration::from_millis(30)).await;
            lock2.release().await;
        }
    });

    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = task1.await;
        let _ = task2.await;
    });
}
