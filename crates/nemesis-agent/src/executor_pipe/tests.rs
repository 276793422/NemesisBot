use super::*;

#[test]
fn pipe_name_format() {
    assert_eq!(pipe_name("abc123"), r"\\.\pipe\NemesisBox_abc123");
    assert_eq!(pipe_name(""), r"\\.\pipe\NemesisBox_");
}

#[test]
fn unique_pipe_id_increments_and_contains_pid() {
    let a = unique_pipe_id();
    let b = unique_pipe_id();
    assert_ne!(a, b, "consecutive ids must be unique");
    let pid = std::process::id().to_string();
    assert!(
        a.starts_with(&pid),
        "id should embed pid: {} vs {}",
        a,
        pid
    );
    assert!(a.contains('_'));
}

/// In-process roundtrip: server side (`create_server` + `connect`) and client
/// side (`connect_client`) exchange one line each way. No sandbox/Start.exe
/// involved — this validates the transport seam standalone (L2.1 style).
#[cfg(windows)]
#[tokio::test]
async fn named_pipe_roundtrip() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let name = pipe_name(&unique_pipe_id());
    let mut server = create_server(&name).expect("create server");

    let client_name = name.clone();
    let client_task = tokio::spawn(async move {
        let mut client = connect_client(&client_name)
            .await
            .expect("client connect");
        let mut buf = [0u8; 5];
        client.read_exact(&mut buf).await.expect("client read");
        assert_eq!(&buf, b"ping\n");
        client.write_all(b"pong\n").await.expect("client write");
    });

    let fut = async {
        server.connect().await.expect("server connect");
        server.write_all(b"ping\n").await.expect("server write");
        let mut buf = [0u8; 5];
        server.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"pong\n");
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), fut)
        .await
        .expect("roundtrip within 5s");

    tokio::time::timeout(std::time::Duration::from_secs(5), client_task)
        .await
        .expect("client task within 5s")
        .expect("client task join");
}

/// A second pipe with a different name must coexist with a live one (server
/// instances are independent per name).
#[cfg(windows)]
#[tokio::test]
async fn two_pipes_are_independent() {
    let name1 = pipe_name(&unique_pipe_id());
    let name2 = pipe_name(&unique_pipe_id());
    let s1 = create_server(&name1).expect("server1");
    let s2 = create_server(&name2).expect("server2");
    // Dropping without connecting is fine; just assert both were created.
    drop(s1);
    drop(s2);

    // Creating a second server instance with the SAME name and
    // first_pipe_instance(true) must fail (single-instance guard).
    let n3 = pipe_name(&unique_pipe_id());
    let a = create_server(&n3).expect("first instance");
    assert!(
        create_server(&n3).is_err(),
        "second first_pipe_instance create must fail"
    );
    drop(a);
}
