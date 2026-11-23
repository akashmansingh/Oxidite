use oxidite_directory::{
    DirectoryError, InMemoryDirectory, PartitionableDirectory, SessionDirectory,
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn test_partition_lease_renewals_and_verify_no_split_brain() {
    let raw_dir = InMemoryDirectory::new();
    let dir = PartitionableDirectory::new(raw_dir);
    let session_id = uuid::Uuid::new_v4();

    // 1. Node 1 claims lease
    let lease_node1 = dir
        .claim(
            session_id,
            "gateway-node-1".into(),
            Duration::from_millis(50),
        )
        .await
        .expect("claim success");
    assert_eq!(lease_node1.fencing_token, 1);
    assert_eq!(lease_node1.node, "gateway-node-1");

    // 2. Network partition strikes Node 1
    dir.set_partitioned(true);

    // Node 1 attempts renewal -> FAILS due to partition
    let renew_attempt = dir.renew(&lease_node1, Duration::from_millis(50)).await;
    assert!(renew_attempt.is_err());

    // 3. Lease expires
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Heal partition from directory perspective so Node 2 can act
    dir.set_partitioned(false);

    // Orphans list shows expired session
    let orphans = dir.list_orphans().await.expect("list orphans ok");
    assert!(orphans.contains(&session_id));

    // 4. Node 2 adopts the orphan session and claims lease with HIGHER fencing token
    let lease_node2 = dir
        .claim(session_id, "gateway-node-2".into(), Duration::from_secs(60))
        .await
        .expect("node2 adopt ok");
    assert_eq!(lease_node2.fencing_token, 2);
    assert_eq!(lease_node2.node, "gateway-node-2");

    // 5. Stale Node 1 attempts to renew or act with its old fencing token (1)
    // MUST BE REJECTED! Split-brain prevented!
    let stale_renew = dir.renew(&lease_node1, Duration::from_secs(60)).await;
    match stale_renew {
        Err(DirectoryError::FencingMismatch { expected, got }) => {
            assert_eq!(expected, 2);
            assert_eq!(got, 1);
        }
        other => panic!("Expected FencingMismatch, got: {:?}", other),
    }

    // Owner is strictly Node 2
    let owner = dir.lookup(session_id).await.unwrap().unwrap();
    assert_eq!(owner.node, "gateway-node-2");
    assert_eq!(owner.fencing_token, 2);
}

#[tokio::test]
async fn test_kill_owner_node_and_verify_orphan_adoption_and_client_resume() {
    let shared_dir = Arc::new(InMemoryDirectory::new());
    let session_id = uuid::Uuid::new_v4();

    // Node 1 claims lease on session with short TTL
    let lease_1 = shared_dir
        .claim(
            session_id,
            "gateway-node-1".into(),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
    assert_eq!(lease_1.node, "gateway-node-1");

    // Node 1 crashes and stops heartbeating
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Node 2 detects orphan
    let orphans = shared_dir.list_orphans().await.unwrap();
    assert!(orphans.contains(&session_id));

    // Node 2 adopts the orphan
    let lease_2 = shared_dir
        .claim(session_id, "gateway-node-2".into(), Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(lease_2.node, "gateway-node-2");
    assert!(lease_2.fencing_token > lease_1.fencing_token);

    // Clients looking up session find Node 2 as owner
    let owner = shared_dir.lookup(session_id).await.unwrap().unwrap();
    assert_eq!(owner.node, "gateway-node-2");
    assert_eq!(owner.fencing_token, lease_2.fencing_token);
}
