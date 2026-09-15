#!/usr/bin/env python3
"""Exercise a packaged Linux GUI through AT-SPI, using disposable configuration.

Run under a dedicated `dbus-run-session -- xvfb-run -a` session with Python GI
and the Atspi 2.0 typelib installed. Storage roots must already be mounted and
writable. This never enrolls, prepares a VM, formats a disk, or changes a VPN.
Native VM qualification is a separate check.
"""
import argparse
import ctypes
import ctypes.util
from contextlib import ExitStack
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import tempfile
import time


def saved_total_locations(saved):
    layout = saved.get('storageLayout')
    assert layout and layout['version'] == 1, 'The saved VM layout is missing'
    primary = [location for location in saved['storageLocations']
               if location['id'] == layout['systemLocationId']]
    assert len(primary) == 1, 'The saved system allocation has no selected location'
    assert layout['volumeId'] == primary[0]['volumeId'], 'The system volume differs from the selection'
    assert PurePosixPath(layout['runtimeDirectory']).parent == PurePosixPath(primary[0]['directory']), 'The VM is outside the picked storage folder'
    assert isinstance(layout['systemGib'], int) and layout['systemGib'] >= 16
    return [(location['directory'], location['allocationGib'] +
             (layout['systemGib'] if location['id'] == layout['systemLocationId'] else 0),
             location['volumeId']) for location in saved['storageLocations']]


def run(args):
    import gi
    gi.require_version('Atspi', '2.0')
    from gi.repository import Atspi
    # Use the X server's supported input API: GTK WebKit exposes editable fields
    # through AT-SPI but does not implement its EditableText interface.
    x11 = ctypes.CDLL(ctypes.util.find_library('X11'))
    xtest = ctypes.CDLL(ctypes.util.find_library('Xtst'))
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x11.XKeysymToKeycode.restype = ctypes.c_uint
    x11.XkbKeycodeToKeysym.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_int]
    x11.XkbKeycodeToKeysym.restype = ctypes.c_ulong
    x11.XFlush.argtypes = [ctypes.c_void_p]
    xtest.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
    display = x11.XOpenDisplay(None)
    if not display:
        raise ValueError('Run GUI acceptance inside its dedicated Xvfb display')

    def physical_key(symbol, pressed):
        code = x11.XKeysymToKeycode(display, symbol)
        if not code or not xtest.XTestFakeKeyEvent(display, code, int(pressed), 0):
            raise AssertionError('The isolated display rejected keyboard input')
        x11.XFlush(display)

    if len(args.storage_root) != len(args.allocation_gib):
        raise ValueError('Provide one --allocation-gib for each --storage-root')
    if args.output.exists():
        raise ValueError('Use a new evidence file; previous results are preserved')
    for binary in [args.app, args.agent]:
        version = subprocess.check_output([str(binary.resolve()), '--version'], text=True, timeout=30)
        if f' {args.version} ({args.commit})' not in version:
            raise ValueError('The packaged executable does not identify the expected version and commit')

    def descendants(node, depth=0):
        if depth > 40:
            return
        yield node
        for i in range(node.get_child_count()):
            child = node.get_child_at_index(i)
            if child is not None:
                yield from descendants(child, depth + 1)

    def control(name, role=None, timeout=45):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                if process is None or process.poll() is not None:
                    raise AssertionError('The isolated packaged app exited before GUI acceptance')
                desktop = Atspi.get_desktop(0)
                applications = [desktop.get_child_at_index(i) for i in range(desktop.get_child_count())]
                owned = [app for app in applications if app is not None and app.get_process_id() == process.pid]
                for node in (node for app in owned for node in descendants(app)):
                    label = node.get_name()
                    # GTK WebKit appends the current value to input names.
                    if (label == name or (role in ('entry', 'combo box') and label.startswith(name + ' '))) and (role is None or node.get_role_name() == role):
                        return node
            except Exception:
                pass  # AT-SPI objects can disappear while React changes pages.
            time.sleep(.1)
        raise AssertionError('Packaged GUI control unavailable: ' + name)

    def click(name):
        button = control(name, 'button')
        if not button.get_state_set().contains(Atspi.StateType.ENABLED):
            raise AssertionError('Packaged GUI button is disabled: ' + name)
        if not button.get_action_iface().do_action(0):
            raise AssertionError('Packaged GUI button could not be activated: ' + name)
        time.sleep(.15)

    def fill(name, value):
        entry = control(name, 'entry')
        if not entry.get_component_iface().grab_focus():
            raise AssertionError('Packaged GUI input cannot receive focus: ' + name)
        physical_key(0xffe3, True)  # Control
        physical_key(ord('a'), True)
        physical_key(ord('a'), False)
        physical_key(0xffe3, False)
        time.sleep(.05)
        for character in str(value):
            symbol = ord(character)
            code = x11.XKeysymToKeycode(display, symbol)
            plain = x11.XkbKeycodeToKeysym(display, code, 0, 0)
            shifted = x11.XkbKeycodeToKeysym(display, code, 0, 1)
            if not code or symbol not in (plain, shifted):
                raise ValueError('The isolated Xvfb keymap cannot type a supplied test path')
            shift = symbol != plain
            if shift:
                physical_key(0xffe1, True)
            physical_key(symbol, True)
            physical_key(symbol, False)
            if shift:
                physical_key(0xffe1, False)
        # XTest input and AT-SPI actions use separate queues. Wait for WebKit
        # to consume every key before an accessibility action changes pages.
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            current = Atspi.Text.get_text(control(name, 'entry'), 0, -1)
            if current == str(value):
                return
            time.sleep(.05)
        raise AssertionError(f'Keyboard input did not reach {name}: {current!r}')

    def key(value):
        physical_key(value, True)
        physical_key(value, False)
        time.sleep(.15)

    with ExitStack() as stack:
        temporary = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix='nodeharbor-gui-')))
        config = temporary / 'config'
        environment = dict(os.environ, NODEHARBOR_CONFIG_DIR=str(config), GDK_BACKEND='x11',
                           XDG_CONFIG_HOME=str(temporary / 'xdg-config'),
                           XDG_DATA_HOME=str(temporary / 'xdg-data'),
                           NO_AT_BRIDGE='0', GTK_MODULES='atk-bridge', XDG_CURRENT_DESKTOP='XFCE')
        directories = [Path(stack.enter_context(tempfile.TemporaryDirectory(prefix='nh-', dir=root)))
                       for root in args.storage_root]
        snapshot = json.loads(subprocess.check_output([str(args.agent.resolve()), '--config-dir', str(config), 'status'], env=environment, text=True, timeout=45))
        eligible = [v for v in snapshot['storage']['volumes'] if v['eligible'] and v['id']]
        selected = []
        for directory in directories:
            matching = [v for v in eligible if directory.resolve().is_relative_to(Path(v['mountPoint']))]
            if not matching:
                raise ValueError('A supplied storage root has no eligible mounted volume')
            selected.append(max(matching, key=lambda v: len(v['mountPoint'])))
        if len(selected) > 1 and len({v['id'] for v in selected}) != len(selected):
            raise ValueError('The multiple-drive check requires distinct mounted volumes')
        log = stack.enter_context((args.output.parent / (args.output.stem + '-app.log')).open('w'))
        process = None

        def start():
            return subprocess.Popen([str(args.app.resolve())], env=environment, stdout=log, stderr=subprocess.STDOUT)

        def stop():
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)

        try:
            process = start()
            click('Sharing rules')
            fill('CPU cores', 1)
            control('Only while idle', 'check box').get_action_iface().do_action(0)
            for index, (directory, volume, allocation) in enumerate(zip(directories, selected, args.allocation_gib), 1):
                click('Add drive')
                save = control('Save sharing rules', 'button')
                if not save.get_state_set().contains(Atspi.StateType.ENABLED):
                    raise AssertionError('Adding a disk disabled Save sharing rules')
                combo = control(f'Drive for disk {index}', 'combo box')
                combo.get_component_iface().grab_focus()
                key(0xff50)  # Home: select the placeholder, then visit eligible options.
                for _ in range(eligible.index(volume) + 1):
                    key(0xff54)  # Down; unsupported options are disabled.
                key(0xff09)  # Tab commits the choice through the native control.
                fill(f'Allocation for disk {index} (GiB)', allocation)
                fill(f'Directory for disk {index}', directory)
            click('Your machine')
            click('Sharing rules')
            for index in range(1, len(directories) + 1):
                control(f'Allocation for disk {index} (GiB)', 'entry')
            click('Save sharing rules')
            control('Save sharing rules and storage?')
            click('Keep editing')
            click('Save sharing rules')
            click('Confirm and save')
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                saved = json.loads((config / 'config.json').read_text())
                if len(saved.get('storageLocations', [])) == len(directories):
                    break
                time.sleep(.2)
            else:
                raise AssertionError('Combined GUI save did not persist the selected disks')
            expected = [(str(p), n, v['id']) for p, n, v in zip(directories, args.allocation_gib, selected)]
            actual = saved_total_locations(saved)
            assert actual == expected, 'The saved locations differ from the GUI choices'
            assert saved['policy']['resources']['diskGib'] == sum(args.allocation_gib)
            assert saved['policy']['resources']['cpus'] == 1 and saved['policy']['idleOnly']
            assert not saved['vmCreated'] and not saved['prepareRequested']
            stop()
            process = start()
            click('Sharing rules')
            for index in range(1, len(directories) + 1):
                control(f'Allocation for disk {index} (GiB)', 'entry')
            reopened = json.loads((config / 'config.json').read_text())
            assert reopened['storageLocations'] == saved['storageLocations']
            assert reopened['storageLayout'] == saved['storageLayout']
            assert reopened['policy'] == saved['policy']
            report = dict(version=args.version, commit=args.commit, packagedGuiPassed=True,
                          navigationDraftPassed=True, combinedSavePassed=True, reopenPassed=True,
                          allocationsGib=args.allocation_gib, nativeRuntimeTested=False)
            with args.output.open('x') as output:
                json.dump(report, output, indent=2)
                output.write('\n')
            print(json.dumps(report), flush=True)
        except Exception:
            # Preserve the actual validation message when a reviewed action
            # fails. Only this disposable application's accessibility tree is read.
            try:
                desktop = Atspi.get_desktop(0)
                for i in range(desktop.get_child_count()):
                    app = desktop.get_child_at_index(i)
                    if app is not None and process is not None and app.get_process_id() == process.pid:
                        labels = [node.get_name() for node in descendants(app) if node.get_name()]
                        print(json.dumps({'failedGuiLabels': labels[:200]}), flush=True)
            except Exception:
                pass
            raise
        finally:
            stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, required=True)
    parser.add_argument('--agent', type=Path, required=True)
    parser.add_argument('--storage-root', type=Path, action='append', required=True)
    parser.add_argument('--allocation-gib', type=int, action='append', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--output', type=Path, required=True)
    run(parser.parse_args())


if __name__ == '__main__':
    main()
