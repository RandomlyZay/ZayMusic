#!/usr/bin/env python3
"""Keep SDK linkage and simulator verification on Apple's native Liquid Glass platform."""
import json
import plistlib
from pathlib import Path
import re
import sys


def deployment_target(project):
    # Read XcodeGen's committed iOS minimum; never maintain a separate Rust default.
    matches = re.findall(r"(?m)^    iOS: ['\"]?(\d+\.\d+(?:\.\d+)?)['\"]?[ \t]*$", Path(project).read_text())
    if len(matches) != 1:
        raise ValueError('Expected one explicit iOS deployment target in the XcodeGen project')
    return matches[0]


def require_modern_sdk(version):
    match = re.match(r'^(?:iphoneos)?(\d+)(?:\.|$)', version)
    if not match or int(match[1]) < 26:
        raise ValueError('Native Liquid Glass requires the iOS 26 or newer SDK; select a current Xcode')


def select_xcode(directory):
    choices = []
    for app in Path(directory).glob('Xcode_*.app'):
        match = re.fullmatch(r'Xcode_(\d+(?:\.\d+)*)\.app', app.name)
        if match and int(match[1].split('.')[0]) >= 26:
            choices.append((tuple(map(int, match[1].split('.'))), app / 'Contents/Developer'))
    if not choices:
        raise ValueError('No stable Xcode 26 or newer is installed')
    return str(max(choices)[1])


def select_simulator(data):
    candidates = []
    for runtime, devices in data.get('devices', {}).items():
        match = re.search(r'\.iOS-(\d+(?:-\d+)*)$', runtime)
        if not match:
            continue
        version = tuple(map(int, match[1].split('-')))
        if version[0] < 26:
            continue
        for device in devices:
            name = device.get('name', '')
            if name.startswith('iPhone') and device.get('isAvailable', True):
                candidates.append((version, name.endswith(' Pro'), name, device['udid']))
    if not candidates:
        raise ValueError('No available iPhone with iOS 26 or newer; native glass screenshots cannot be verified')
    return max(candidates)[-1]


def verify_app(directory):
    root = Path(directory)
    with (root / 'Info.plist').open('rb') as f:
        info = plistlib.load(f)
    require_modern_sdk(info.get('DTSDKName', ''))
    if info.get('UIDesignRequiresCompatibility', False):
        raise ValueError('The app opts out of the native system design')
    if 'audio' not in info.get('UIBackgroundModes', []):
        raise ValueError('Background audio mode is missing')
    if not (root / info.get('CFBundleExecutable', 'missing-executable')).is_file():
        raise ValueError('Device executable is missing')
    if (root / '_CodeSignature').exists():
        raise ValueError('LiveContainer delivery must remain unsigned')


def self_test():
    import tempfile
    import unittest

    class PlatformTests(unittest.TestCase):
        def test_shared_deployment_target(self):
            with tempfile.TemporaryDirectory() as tmp:
                project = Path(tmp) / 'project.yml'
                project.write_text("options:\n  deploymentTarget:\n    iOS: '17.0'\n")
                self.assertEqual(deployment_target(project), '17.0')
                project.write_text("options:\n  deploymentTarget:\n    iOS: '26.0'\n")
                self.assertEqual(deployment_target(project), '26.0')
                project.write_text('options: {}\n')
                with self.assertRaises(ValueError):
                    deployment_target(project)

        def test_sdk_linkage_gate(self):
            for version in ['26.0', '26.5', 'iphoneos27.0']:
                require_modern_sdk(version)
            for version in ['18.5', 'iphoneos18.5', '']:
                with self.assertRaises(ValueError):
                    require_modern_sdk(version)

        def test_xcode_selects_latest_stable_and_excludes_preview_names(self):
            with tempfile.TemporaryDirectory() as tmp:
                for name in ['Xcode_16.4.app', 'Xcode_26.6.app', 'Xcode_27.0.app', 'Xcode_27.1_beta.app']:
                    (Path(tmp) / name / 'Contents/Developer').mkdir(parents=True)
                self.assertEqual(select_xcode(tmp), str(Path(tmp) / 'Xcode_27.0.app/Contents/Developer'))
            with tempfile.TemporaryDirectory() as tmp:
                with self.assertRaises(ValueError):
                    select_xcode(tmp)

        def test_simulator_uses_newest_available_modern_runtime(self):
            def phone(name, identity, available=True):
                return {'name': name, 'udid': identity, 'isAvailable': available}
            data = {'devices': {
                'com.apple.CoreSimulator.SimRuntime.iOS-18-5': [phone('iPhone 16 Pro', 'old')],
                'com.apple.CoreSimulator.SimRuntime.iOS-26-2': [phone('iPhone 17 Pro', 'earlier')],
                'com.apple.CoreSimulator.SimRuntime.iOS-26-5': [phone('iPhone Air', 'air'), phone('iPhone 17 Pro', 'modern')],
                'com.apple.CoreSimulator.SimRuntime.iOS-27-0': [phone('iPhone 18 Pro', 'unavailable', False)],
                'com.apple.CoreSimulator.SimRuntime.tvOS-27-0': [phone('iPhone misleading fixture', 'wrong-platform')],
            }}
            self.assertEqual(select_simulator(data), 'modern')
            with self.assertRaises(ValueError):
                select_simulator({'devices': {'com.apple.CoreSimulator.SimRuntime.iOS-18-5': [phone('iPhone 16', 'old')]}})

        def test_artifact_rejects_old_sdk_and_legacy_design(self):
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                (root / 'ZayMusic').write_bytes(b'fixture')
                info = {'DTSDKName': 'iphoneos26.5', 'CFBundleExecutable': 'ZayMusic', 'UIBackgroundModes': ['audio']}
                def save():
                    (root / 'Info.plist').write_bytes(plistlib.dumps(info))
                save()
                verify_app(root)
                info['DTSDKName'] = 'iphoneos18.5'
                save()
                with self.assertRaises(ValueError):
                    verify_app(root)
                info['DTSDKName'] = 'iphoneos27.0'
                info['UIDesignRequiresCompatibility'] = True
                save()
                with self.assertRaises(ValueError):
                    verify_app(root)

    result = unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(PlatformTests))
    if not result.wasSuccessful():
        raise SystemExit(1)


if __name__ == '__main__':
    try:
        mode = sys.argv[1]
        if mode == 'deployment-target':
            print(deployment_target(Path(__file__).resolve().parents[1] / 'apps/ios/project.yml'))
        elif mode == 'sdk':
            require_modern_sdk(sys.argv[2])
            print('Modern iOS SDK verified:', sys.argv[2])
        elif mode == 'xcode':
            print(select_xcode(sys.argv[2] if len(sys.argv) > 2 else '/Applications'))
        elif mode == 'simulator':
            print(select_simulator(json.load(sys.stdin)))
        elif mode == 'app':
            verify_app(sys.argv[2])
            print('Unsigned app verified: modern SDK linkage, native design and background audio')
        elif mode == 'self-test':
            self_test()
        else:
            raise ValueError('Unknown platform-check mode')
    except (ValueError, IndexError) as exc:
        raise SystemExit(str(exc))
