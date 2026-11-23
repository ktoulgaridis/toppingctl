# Linux: access the E2x2 without sudo

On Linux, toppingctl talks to the TOPPING E2x2 through its hidraw device node
(`/dev/hidrawN`), which is root-only by default. The udev rule
[`70-toppingctl.rules`](70-toppingctl.rules) gives that node to the user logged in
at the local seat (`TAG+="uaccess"`). It sets no `MODE` and no `GROUP`, so the device
never becomes world-writable and no group is granted access. Remote (SSH) sessions
without a seat do not get access.

Requires systemd-logind (standard on most desktop distributions).

## Install

From the repository root, or from the unpacked release archive (Linux archives
include `packaging/linux/`):

```sh
sudo install -m 0644 packaging/linux/70-toppingctl.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger
```

Then unplug the E2x2 and plug it back in (replug), so the rule applies to a fresh
device node.

Check it worked: `getfacl /dev/hidrawN` (the E2x2's node) should list your user.

## Uninstall

```sh
sudo rm /etc/udev/rules.d/70-toppingctl.rules
sudo udevadm control --reload
sudo udevadm trigger
```

Then replug the E2x2; its node is root-only again.
