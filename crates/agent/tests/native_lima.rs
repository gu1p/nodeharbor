#![cfg(target_os = "macos")]
use anyhow::{Context, Result};
use nodeharbor_agent::{guest_files, LimaRunner, Runner, Vm};
use nodeharbor_core::Resources;
use std::{path::PathBuf, sync::Arc};

#[tokio::test]
#[ignore = "Boots an isolated real VM; requires NODEHARBOR_TEST_LIMA pointing to the pinned native runtime"]
async fn native_macos_worker_boots_updates_resizes_and_recovers_in_application_data() -> Result<()>
{
    let binary = PathBuf::from(
        std::env::var("NODEHARBOR_TEST_LIMA")
            .context("Set the native acceptance test's Lima binary")?,
    );
    let base = dirs::config_dir().context("No application data directory")?;
    let directory = tempfile::Builder::new()
        .prefix("nodeharbor-test-")
        .tempdir_in(base)?;
    let identity = uuid::Uuid::new_v4().to_string();
    let runner = Arc::new(LimaRunner::new(binary, directory.path().join("lima")));
    let vm = Vm::managed(&identity, directory.path(), runner.clone())?;
    let resources = Resources {
        cpus: 2,
        memory_mib: 2048,
        disk_gib: 8,
    };
    let result:Result<()>=async {
        vm.create(&resources,directory.path(),guest_files()).await?;
        anyhow::ensure!(vm.info().await?.reachable,"The VM is not reachable");
        vm.verify_owner().await?;
        let script=r#"import json, os, pathlib, stat, subprocess, urllib.request
owner=pathlib.Path('/etc/nodeharbor/device-id')
assert stat.S_IMODE(owner.stat().st_mode)==0o600
assert os.cpu_count()==2
memory=int(pathlib.Path('/proc/meminfo').read_text().splitlines()[0].split()[1])
assert 1800*1024 < memory <= 2048*1024
for file in ['configure_worker.py','watchdog.py']:
    assert pathlib.Path('/usr/local/lib/nodeharbor',file).is_file()
mounts=subprocess.run(['findmnt','-rn','-t','9p,virtiofs'],capture_output=True)
assert mounts.returncode==1 and not mounts.stdout
with urllib.request.urlopen('https://api.github.com/zen',timeout=20) as response:assert response.status==200
path=pathlib.Path('/usr/local/lib/nodeharbor/watchdog.py')
with path.open('a') as file:file.write('\n# native-update-survives-reboot\n')
print('Guest budget, ownership, files, filesystem isolation, HTTPS and update: passed')
"#;
        let output=runner.run(&["shell".into(),"--workdir=/".into(),"worker".into(),"sudo".into(),"python3".into(),"-".into()],Some(script.as_bytes().to_vec()),60).await?;
        anyhow::ensure!(output.success,"Guest inspection failed: {}",output.stderr);
        println!("{}",output.stdout.trim());
        vm.renew_lease().await?;
        vm.stop().await?;
        vm.resize(&Resources {cpus:1,memory_mib:1536,disk_gib:10}).await?;
        vm.start().await?;
        vm.verify_owner().await?;
        let script=r#"import os,pathlib,subprocess,urllib.request
assert os.cpu_count()==1
memory=int(pathlib.Path('/proc/meminfo').read_text().splitlines()[0].split()[1])
assert 1300*1024 < memory <= 1536*1024
root=subprocess.check_output(['findmnt','-nro','SOURCE','/'],text=True).strip()
parent=subprocess.check_output(['lsblk','-nro','PKNAME',root],text=True).strip()
size=int(subprocess.check_output(['blockdev','--getsize64','/dev/'+parent],text=True))
assert size==10*1024**3
assert '# native-update-survives-reboot' in pathlib.Path('/usr/local/lib/nodeharbor/watchdog.py').read_text()
with urllib.request.urlopen('https://api.github.com/zen',timeout=20) as response:assert response.status==200
print('Restart applied CPU, memory and disk changes; preserved the guest update and HTTPS access')
"#;
        let output=runner.run(&["shell".into(),"--workdir=/".into(),"worker".into(),"sudo".into(),"python3".into(),"-".into()],Some(script.as_bytes().to_vec()),60).await?;
        anyhow::ensure!(output.success,"Restart inspection failed: {}",output.stderr);
        println!("{}",output.stdout.trim());
        vm.stop_now().await?;
        anyhow::ensure!(vm.info().await?.stopped,"Stop was not confirmed");
        Ok(())
    }.await;
    // Preserve ownership evidence if native cleanup fails; never leave an
    // unidentified VM behind because a test assertion failed.
    let cleanup: Result<()> = async {
        if vm.has_receipt()? {
            if vm.info().await?.running {
                vm.stop_now().await?;
            }
            vm.remove().await?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = cleanup {
        let path = directory.keep();
        anyhow::bail!(
            "Acceptance result: {result:?}; cleanup failed: {error}; ownership retained at {}",
            path.display()
        );
    }
    result?;
    println!("Native Lima acceptance passed; its isolated VM was stopped and removed");
    Ok(())
}

#[tokio::test]
#[ignore = "Interrupts an isolated real VM boot; requires NODEHARBOR_TEST_LIMA"]
async fn stopping_during_native_preparation_stops_the_owned_vm() -> Result<()> {
    let binary = PathBuf::from(std::env::var("NODEHARBOR_TEST_LIMA")?);
    let directory = tempfile::Builder::new()
        .prefix("nodeharbor-stop-")
        .tempdir_in(dirs::config_dir().context("No app data")?)?;
    let runner = Arc::new(LimaRunner::new(binary, directory.path().join("lima")));
    let agent = nodeharbor_agent::Agent::open_with_runner(directory.path(), runner)?;
    agent.store.update(|config| {
        config.vm_provider = nodeharbor_agent::VmProvider::Lima;
        config.format_version = 2;
        config.device_token = Some("native-test-credential-never-sent".into());
        config.controller_url = Some("http://127.0.0.1:9".into());
        config.prepare_requested = true;
        config.policy.resources = Resources {
            cpus: 1,
            memory_mib: 2048,
            disk_gib: 15,
        };
        Ok(())
    })?;
    let vm = agent.local_vm()?;
    let supervisor = agent.clone();
    let mut operation = tokio::spawn(async move { supervisor.tick().await });
    let result: Result<()> = async {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        while !vm.info().await?.running {
            anyhow::ensure!(
                !operation.is_finished(),
                "Preparation ended before cancellation could be tested"
            );
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "Native VM did not start"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        agent.action("stop").await?;
        tokio::time::timeout(std::time::Duration::from_secs(90), &mut operation).await???;
        anyhow::ensure!(
            vm.info().await?.stopped,
            "Cancelled preparation left a running VM"
        );
        let saved = agent.store.load()?;
        anyhow::ensure!(
            !saved.vm_configured && !saved.prepare_requested && !saved.policy.enabled,
            "Cancellation changed contribution intent"
        );
        anyhow::ensure!(
            !serde_json::to_string(&agent.activity())?
                .contains("native-test-credential-never-sent"),
            "Activity exposed the test credential"
        );
        Ok(())
    }
    .await;
    if !operation.is_finished() {
        operation.abort();
        let _ = operation.await;
    }
    let cleanup: Result<()> = async {
        if vm.has_receipt()? {
            if vm.info().await?.running {
                vm.stop_now().await?;
            }
            vm.remove().await?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = cleanup {
        let path = directory.keep();
        anyhow::bail!(
            "{result:?}; cleanup failed: {error}; receipt kept at {}",
            path.display()
        );
    }
    result?;
    println!("Native preparation cancellation passed; VM stopped and removed");
    Ok(())
}
