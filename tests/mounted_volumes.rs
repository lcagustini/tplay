//! Tests for Volume::parse_mounts — local block partitions only.
//! `parse_mounts_with_labels` is used with an injected label map so the
//! tests are hermetic (the real `/dev/disk/by-label/` is machine-specific).

use std::collections::HashMap;
use tplay::library::Volume;

/// Matches this machine's `/dev/disk/by-label/` (used by `parse_real_mount_table`).
fn real_labels() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("sdc1".into(), "25-ssd-2".into());
    m.insert("sdb1".into(), "25-ssd-3".into());
    m.insert("sdd1".into(), "ssd".into());
    m.insert("sda2".into(), "35-hdd".into());
    m.insert("nvme0n1p2".into(), "endeavouros".into());
    m
}

const FIXTURE_REAL: &str = r#"/dev/nvme0n1p2 / ext4 rw,noatime 0 0
devtmpfs /dev devtmpfs rw,nosuid,size=16335536k,nr_inodes=4083884,mode=755,inode64,huge=advise 0 0
tmpfs /dev/shm tmpfs rw,nosuid,nodev,inode64,huge=advise,usrquota 0 0
devpts /dev/pts devpts rw,nosuid,noexec,relatime,gid=5,mode=600,ptmxmode=000 0 0
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
securityfs /sys/kernel/security securityfs rw,nosuid,nodev,noexec,relatime 0 0
cgroup2 /sys/fs/cgroup cgroup2 rw,nosuid,nodev,noexec,relatime,nsdelegate,memory_recursiveprot,memory_hugetlb_accounting 0 0
none /sys/fs/pstore pstore rw,nosuid,nodev,noexec,relatime 0 0
efivarfs /sys/firmware/efi/efivars efivarfs rw,nosuid,nodev,noexec,relatime 0 0
bpf /sys/fs/bpf bpf rw,nosuid,nodev,noexec,relatime,mode=700 0 0
configfs /sys/kernel/config configfs rw,nosuid,nodev,noexec,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
tmpfs /run tmpfs rw,nosuid,nodev,size=6553236k,nr_inodes=819200,mode=755,inode64,huge=advise 0 0
systemd-1 /proc/sys/fs/binfmt_misc autofs rw,relatime,fd=43,pgrp=1,timeout=0,minproto=5,maxproto=5,direct,pipe_ino=7561 0 0
debugfs /sys/kernel/debug debugfs rw,nosuid,nodev,noexec,relatime 0 0
mqueue /dev/mqueue mqueue rw,nosuid,nodev,noexec,relatime 0 0
hugetlbfs /sys/hugepages hugetlbfs rw,nosuid,nodev,relatime,pagesize=2M 0 0
tracefs /sys/kernel/tracing tracefs rw,nosuid,nodev,noexec,relatime 0 0
fusectl /sys/fs/fuse/connections fusectl rw,nosuid,nodev,noexec,relatime 0 0
/dev/sdc1 /mnt/9b1c255b-05ea-4060-afa0-307f6389c216 ext4 rw,nosuid,nodev,relatime 0 0
/dev/sdb1 /mnt/1033f5c3-935a-42aa-ae9c-759653a1afa0 ext4 rw,nosuid,nodev,relatime 0 0
/dev/sdd1 /mnt/3b95d255-4dce-4708-a53f-a803f47f56b9 ext4 rw,nosuid,nodev,relatime 0 0
/dev/nvme0n1p1 /efi vfat rw,relatime,fmask=0137,dmask=0027,codepage=437,iocharset=ascii,shortname=mixed,utf8,errors=remount-ro 0 0
binfmt_misc /proc/sys/fs/binfmt_misc binfmt_misc rw,nosuid,nodev,noexec,relatime 0 0
/dev/sda2 /mnt/9C4ABB604ABB35BC ntfs3 rw,nosuid,nodev,relatime,uid=0,gid=0,acl,iocharset=utf8,prealloc 0 0
tmpfs /run/user/1000 tmpfs rw,nosuid,nodev,relatime,size=3276616k,nr_inodes=819154,mode=700,uid=1000,gid=1001,inode64,huge=advise 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw,nosuid,nodev,relatime,user_id=1000,group_id=1001 0 0
portal /run/user/1000/doc fuse.portal rw,nosuid,nodev,relatime,user_id=1000,group_id=1001 0 0
"#;

const FIXTURE_SYNTHETIC: &str = r#"/dev/nvme0n1p2 / ext4 rw,noatime 0 0
/dev/sda1 /mnt/data ext4 rw 0 0
/dev/sdb1 /mnt/backup xfs rw 0 0
192.168.1.100:/export /mnt/nfs nfs4 rw 0 0
//server/share /mnt/smb cifs rw 0 0
sshfs#user@host:/remote /mnt/ssh fuse.sshfs rw 0 0
devtmpfs /dev devtmpfs rw 0 0
tmpfs /tmp tmpfs rw 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0
"#;

#[test]
fn parse_real_mount_table_uses_disk_labels() {
    let vols = Volume::parse_mounts_with_labels(FIXTURE_REAL, &real_labels());
    let labels: Vec<String> = vols.iter().map(|v| v.label.clone()).collect();
    // Pretty filesystem labels win over the UUID mountpoint leaf names.
    assert!(labels.contains(&"25-ssd-2".to_string()));
    assert!(labels.contains(&"25-ssd-3".to_string()));
    assert!(labels.contains(&"ssd".to_string()));
    assert!(labels.contains(&"35-hdd".to_string()));
    assert!(labels.contains(&"endeavouros".to_string()));
    // Boot/ESP partition (/efi) is excluded — not a user volume.
    assert!(!labels.contains(&"efi".to_string()));
    // No pseudo-fs entries
    assert!(!labels.iter().any(|l| l == "dev"
        || l == "sys"
        || l == "proc"
        || l == "run"
        || l == "tmp"
        || l == "shm"));
    // No GVFS/fuse entries
    assert!(!labels.iter().any(|l| l == "gvfs" || l == "doc"));
}

#[test]
fn parse_synthetic_mount_table_excludes_network() {
    let vols = Volume::parse_mounts_with_labels(FIXTURE_SYNTHETIC, &HashMap::new());
    let labels: Vec<String> = vols.iter().map(|v| v.label.clone()).collect();
    // Local block devices only, mountpoint leaf as label (no disk labels injected).
    assert!(labels.contains(&"data".to_string()));
    assert!(labels.contains(&"backup".to_string()));
    assert!(labels
        .iter()
        .any(|l| l == "nvme0n1p2" || l.starts_with("nvme0n1p")));
    // Network mounts excluded
    assert!(!labels.contains(&"nfs".to_string()));
    assert!(!labels.contains(&"smb".to_string()));
    assert!(!labels.contains(&"ssh".to_string()));
    // Pseudo-fs excluded
    assert!(!labels.contains(&"dev".to_string()));
    assert!(!labels.contains(&"tmp".to_string()));
    assert!(!labels.contains(&"gvfs".to_string()));
}

#[test]
fn boot_partitions_are_excluded() {
    let fixture = r#"/dev/nvme0n1p2 / ext4 rw,noatime 0 0
/dev/nvme0n1p1 /efi vfat rw,relatime 0 0
/dev/sda1 /boot ext4 rw 0 0
/dev/sdb1 /boot/efi vfat rw 0 0
/dev/sdc1 /mnt/music ext4 rw 0 0
"#;
    let vols = Volume::parse_mounts_with_labels(fixture, &HashMap::new());
    let labels: Vec<String> = vols.iter().map(|v| v.label.clone()).collect();
    // Root + the data mount survive.
    assert!(labels
        .iter()
        .any(|l| l == "nvme0n1p2" || l.starts_with("nvme0n1p")));
    assert!(labels.contains(&"music".to_string()));
    // All three boot/ESP mountpoints are excluded.
    assert!(!labels.contains(&"efi".to_string()));
    assert!(!labels.contains(&"boot".to_string()));
}

#[test]
fn volumes_sorted_by_label() {
    let vols = Volume::parse_mounts_with_labels(FIXTURE_REAL, &real_labels());
    let labels: Vec<String> = vols.iter().map(|v| v.label.clone()).collect();
    // Should be sorted case-insensitively
    let mut sorted = labels.clone();
    sorted.sort_by_key(|a| a.to_lowercase());
    assert_eq!(labels, sorted);
}

#[test]
fn volume_path_is_mountpoint() {
    let vols = Volume::parse_mounts_with_labels(FIXTURE_SYNTHETIC, &HashMap::new());
    for v in vols {
        // For /mnt/* paths, label equals the mountpoint leaf (no disk labels)
        if v.path.starts_with("/mnt/") {
            assert_eq!(
                v.path,
                std::path::PathBuf::from(format!("/mnt/{}", v.label))
            );
        } else {
            // Root "/" gets device basename as label but path is "/"
            assert_eq!(v.path, std::path::PathBuf::from("/"));
        }
    }
}
