//! Native acceptance entry point. Requires an explicitly prepared, isolated,
//! enrolled lab worker; never discovers or adopts an owner's ordinary worker.
use anyhow::{Context, Result};
use nodeharbor_agent::{
    storage::Selection, storage_lifecycle::ReviewOptions, Agent, Runner, Vm, VmProvider,
};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::test]
#[ignore = "Replaces storage only in NODEHARBOR_NATIVE_STORAGE_LIFECYCLE, an enrolled nh-storage-lifecycle-* lab fixture; requires native runtime and controller"]
async fn native_shrink_preserves_linux_metadata_and_confirms_kubernetes_capacity() -> Result<()> {
    let directory =
        PathBuf::from(std::env::var("NODEHARBOR_NATIVE_STORAGE_LIFECYCLE")?).canonicalize()?;
    anyhow::ensure!(
        directory
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("nh-storage-lifecycle-")),
        "Use a dedicated nh-storage-lifecycle-* lab worker"
    );
    let config = nodeharbor_agent::Store::open(&directory)?.load()?;
    anyhow::ensure!(
        config.vm_configured
            && config.device_token.is_some()
            && config.storage_lifecycle.maintenance.is_none(),
        "Prepare and enroll the isolated lab worker first"
    );
    let runner: Arc<dyn Runner> = match config.vm_provider {
        VmProvider::Lima => Arc::new(nodeharbor_agent::LimaRunner::new(
            PathBuf::from(std::env::var("NODEHARBOR_TEST_LIMA")?),
            directory.join("lima"),
        )),
        VmProvider::Multipass => Arc::new(nodeharbor_agent::MultipassRunner),
    };
    let vm = Vm::managed(&config.device_id, &directory, runner.clone())?;
    anyhow::ensure!(
        vm.has_receipt()? && vm.info().await?.reachable,
        "The receipt-owned lab worker must already be running"
    );
    vm.verify_owner().await?;
    let root = if config.vm_provider == VmProvider::Lima {
        "/var/lib/nodeharbor/storage/native-proof"
    } else {
        "/var/lib/rancher/k3s/native-proof"
    };
    let mut args = if config.vm_provider == VmProvider::Lima {
        vec!["shell".to_owned(), "--workdir=/".into(), vm.name.clone()]
    } else {
        vec!["exec".into(), vm.name.clone(), "--".into()]
    };
    args.extend(["sudo","python3","-c", "import os,pathlib,sys\np=pathlib.Path(sys.argv[1]);p.mkdir(exist_ok=False)\nf=p/'payload';f.write_bytes(b'preserved worker bytes');f.chmod(0o640);os.chown(f,12345,23456);os.setxattr(f,'user.nodeharbor',b'preserved metadata');os.link(f,p/'hardlink');(p/'symlink').symlink_to('payload')\nwith (p/'sparse').open('wb') as h: h.seek(1024**3);h.write(b'end')",root].into_iter().map(str::to_owned));
    anyhow::ensure!(
        runner.run(&args, None, 60).await?.success,
        "Cannot seed the native preservation proof"
    );
    let agent = Agent::open_with_runner(&directory, runner.clone())?;
    let total = config.policy.resources.disk_gib;
    anyhow::ensure!(
        total >= 30,
        "The lab worker needs at least 30 GiB before this shrink test"
    );
    let target = total / 2;
    let plan = if config.vm_provider == VmProvider::Lima {
        let first = config
            .storage_locations
            .first()
            .context("Native Lima proof requires a prepared pool")?;
        agent
            .preview_storage(vec![Selection {
                expected_volume_id: None,
                id: Some(first.id.clone()),
                directory: first.directory.clone(),
                allocation_gib: target,
            }])
            .await?
    } else {
        agent
            .preview_storage_request(
                vec![],
                ReviewOptions {
                    single_disk_gib: Some(target),
                    ..Default::default()
                },
            )
            .await?
    };
    agent.apply_storage(plan).await?;
    let outcome=tokio::time::timeout(Duration::from_secs(7200),async {
        loop {
            let saved=agent.store.load()?;
            if saved.storage_lifecycle.maintenance.is_none() {break;}
            agent.tick().await?;
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        let saved=agent.store.load()?;
        anyhow::ensure!(saved.policy.resources.disk_gib==target && saved.storage_generation>config.storage_generation,"Native storage capacity/generation did not change");
        vm.start().await?;vm.verify_owner().await?;vm.renew_lease().await?;
        let mut verify=args[..args.len()-2].to_vec();
        verify.extend(["import os,pathlib,sys\np=pathlib.Path(sys.argv[1]);f=p/'payload'\nassert f.read_bytes()==b'preserved worker bytes'\nassert f.stat().st_uid==12345 and f.stat().st_gid==23456 and f.stat().st_mode&0o777==0o640\nassert f.stat().st_ino==(p/'hardlink').stat().st_ino\nassert os.readlink(p/'symlink')=='payload'\nassert os.getxattr(f,'user.nodeharbor')==b'preserved metadata'\nassert (p/'sparse').stat().st_blocks*512<1024**2\nwith (p/'sparse').open('rb') as h:h.seek(1024**3);assert h.read()==b'end'",root].into_iter().map(str::to_owned));
        anyhow::ensure!(runner.run(&verify,None,60).await?.success,"Native restored data or metadata differs");
        vm.replacement_capacity(target).await?;
        Ok::<_,anyhow::Error>(())
    }).await;
    if vm.has_receipt()? && vm.info().await?.running {
        vm.stop_now().await?;
    }
    outcome.context("Native maintenance timed out; its journal and backup are retained")?
}
