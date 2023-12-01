use std::io::Write;
use tracing::*;

use futures::{join, stream, AsyncBufReadExt, StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;

use kube::{
    api::{
        Api, AttachParams, AttachedProcess, DeleteParams, LogParams, PostParams, ResourceExt,
        WatchEvent, WatchParams,
    },
    Client,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let client = Client::try_default().await?;

    info!("Creating a Pod that outputs \"Hello, kube-rs!\"");
    let p: Pod = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": "test-kane8n" },
        "spec": {
            "containers": [{
                "name": "test-kane8n",
                "image": "alpine",
                "command": ["sh", "-c", "echo \"Hello, kube-rs!\" && sleep 10"],
            }],
        }
    }))?;

    let pods: Api<Pod> = Api::default_namespaced(client);
    // Stop on error including a pod already exists or is still being deleted.
    pods.create(&PostParams::default(), &p).await?;

    // Wait until the pod is running, otherwise we get 500 error.
    let wp = WatchParams::default()
        .fields("metadata.name=test-kane8n")
        .timeout(10);
    let mut stream = pods.watch(&wp, "0").await?.boxed();
    while let Some(status) = stream.try_next().await? {
        match status {
            WatchEvent::Added(o) => {
                info!("Added {}", o.name_any());
            }
            WatchEvent::Modified(o) => {
                let s = o.status.as_ref().expect("status exists on pod");
                if s.phase.clone().unwrap_or_default() == "Running" {
                    info!("Ready to attach to {}", o.name_any());
                    break;
                }
            }
            _ => {}
        }
    }

    let ap = AttachParams::default();
    // Attach to see numbers printed on stdout.
    let attached = pods.attach("test-kane8n", &ap).await?;
    // Separate stdout/stderr outputs
    // separate_outputs(attached).await;
    // Combining stdout and stderr output.
    combined_output(attached).await;

    info!("Fetching logs for test-kane8n");
    let mut logs = pods
        .log_stream(
            "test-kane8n",
            &LogParams {
                follow: true,
                container: None,
                tail_lines: None,
                since_seconds: None,
                timestamps: false,
                ..LogParams::default()
            },
        )
        .await?
        .lines();

    while let Some(line) = logs.try_next().await? {
        println!("{}", line);
    }

    // Delete it
    pods.delete("test-kane8n", &DeleteParams::default())
        .await?
        .map_left(|pdel| {
            assert_eq!(pdel.name_any(), "test-kane8n");
        });

    Ok(())
}

#[allow(dead_code)]
async fn separate_outputs(mut attached: AttachedProcess) {
    let stdout = tokio_util::io::ReaderStream::new(attached.stdout().unwrap());
    let stdouts = stdout.for_each(|res| async {
        if let Ok(bytes) = res {
            let out = std::io::stdout();
            out.lock().write_all(&bytes).unwrap();
        }
    });
    let stderr = tokio_util::io::ReaderStream::new(attached.stderr().unwrap());
    let stderrs = stderr.for_each(|res| async {
        if let Ok(bytes) = res {
            let out = std::io::stderr();
            out.lock().write_all(&bytes).unwrap();
        }
    });

    join!(stdouts, stderrs);
    if let Some(status) = attached.take_status().unwrap().await {
        info!("{:?}", status);
    }
}

#[allow(dead_code)]
async fn combined_output(mut attached: AttachedProcess) {
    let stdout = tokio_util::io::ReaderStream::new(attached.stdout().unwrap());
    let stderr = tokio_util::io::ReaderStream::new(attached.stderr().unwrap());
    let outputs = stream::select(stdout, stderr).for_each(|res| async {
        if let Ok(bytes) = res {
            let out = std::io::stdout();
            out.lock().write_all(&bytes).unwrap();
        }
    });
    outputs.await;
    if let Some(status) = attached.take_status().unwrap().await {
        info!("{:?}", status);
    }
}
