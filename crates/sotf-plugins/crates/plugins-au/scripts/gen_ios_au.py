#!/usr/bin/env python3
"""Generate iOS AUv3 wrappers from desktop AU wrappers + patch project-ios.yml."""
import plistlib
import re
import sys
from pathlib import Path

AU = Path(__file__).resolve().parent.parent
YML = AU / "project-ios.yml"


def write_if_different(path, content):
    try:
        if path.read_text() == content:
            return False
    except FileNotFoundError:
        pass
    path.write_text(content)
    return True

SKIP = {"EQiOSAudioUnit"}  # already exists

UNIT_SWIFT = """#if os(iOS)
import AVFoundation

public final class {unit_class}: GenericRustAudioUnit {{
    override public class var pluginType: String {{ "{plugin_type}" }}
    override public class var pluginSubtype: String {{ "{subtype}" }}
    override public class var pluginName: String {{ "{plugin_name}" }}
}}
#endif
"""

VC_SWIFT = """#if os(iOS)
import AudioToolbox
import CoreAudioKit
import UIKit

public final class {vc_class}: GenericRustiOSViewController, AUAudioUnitFactory {{
    public override class var pluginType: String {{ "{plugin_type}" }}

    public nonisolated func createAudioUnit(
        with componentDescription: AudioComponentDescription
    ) throws -> AUAudioUnit {{
        let unit = try {unit_class}(componentDescription: componentDescription, options: [])
        audioUnit = unit
        return unit
    }}
}}
#endif
"""


def parse_desktop(d):
    base = d.name.removesuffix("AudioUnit")
    swift = d / f"{d.name}.swift"
    text = swift.read_text()
    pt = re.search(r'pluginType: String \{ "([^"]+)" \}', text).group(1)
    st = re.search(r'pluginSubtype: String \{ "([^"]+)" \}', text).group(1)
    pn = re.search(r'pluginName: String \{ "([^"]+)" \}', text).group(1)
    with open(d / "Info.plist", "rb") as f:
        pl = plistlib.load(f)
    comp = pl["NSExtension"]["NSExtensionAttributes"]["AudioComponents"][0]
    return {
        "base": base, "plugin_type": pt, "subtype": st, "plugin_name": pn,
        "display": pl["CFBundleDisplayName"],
        "description": comp["description"],
        "comp_name": comp["name"],
        "tags": comp.get("tags", ["Effects"]),
        "comp_type": comp.get("type", "aufx"),
        "short_version": pl.get("CFBundleShortVersionString", "0.8.2"),
    }


def plist_for(m, target):
    doc = f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
\t<key>CFBundleDevelopmentRegion</key>
\t<string>$(DEVELOPMENT_LANGUAGE)</string>
\t<key>CFBundleDisplayName</key>
\t<string>{m['display']}</string>
\t<key>CFBundleExecutable</key>
\t<string>$(EXECUTABLE_NAME)</string>
\t<key>CFBundleIdentifier</key>
\t<string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>
\t<key>CFBundleInfoDictionaryVersion</key>
\t<string>6.0</string>
\t<key>CFBundleName</key>
\t<string>$(PRODUCT_NAME)</string>
\t<key>CFBundlePackageType</key>
\t<string>$(PRODUCT_BUNDLE_PACKAGE_TYPE)</string>
\t<key>CFBundleShortVersionString</key>
\t<string>{m['short_version']}</string>
\t<key>CFBundleVersion</key>
\t<string>$(CURRENT_PROJECT_VERSION)</string>
\t<key>NSExtension</key>
\t<dict>
\t\t<key>NSExtensionAttributes</key>
\t\t<dict>
\t\t\t<key>AudioComponents</key>
\t\t\t<array>
\t\t\t\t<dict>
\t\t\t\t\t<key>description</key>
\t\t\t\t\t<string>{m['description']}</string>
\t\t\t\t\t<key>factoryFunction</key>
\t\t\t\t\t<string>{target}.{m['base']}iOSViewController</string>
\t\t\t\t\t<key>manufacturer</key>
\t\t\t\t\t<string>SOTF</string>
\t\t\t\t\t<key>name</key>
\t\t\t\t\t<string>{m['comp_name']}</string>
\t\t\t\t\t<key>sandboxSafe</key>
\t\t\t\t\t<true/>
\t\t\t\t\t<key>subtype</key>
\t\t\t\t\t<string>{m['subtype']}</string>
\t\t\t\t\t<key>tags</key>
\t\t\t\t\t<array>
"""
    for t in m["tags"]:
        doc += f"\t\t\t\t\t\t<string>{t}</string>\n"
    doc += f"""\t\t\t\t\t</array>
\t\t\t\t\t<key>type</key>
\t\t\t\t\t<string>{m['comp_type']}</string>
\t\t\t\t\t<key>version</key>
\t\t\t\t\t<integer>1</integer>
\t\t\t\t</dict>
\t\t\t</array>
\t\t</dict>
\t\t<key>NSExtensionPointIdentifier</key>
\t\t<string>com.apple.AudioUnit-UI</string>
\t\t<key>NSExtensionPrincipalClass</key>
\t\t<string>$(PRODUCT_MODULE_NAME).{m['base']}iOSViewController</string>
\t</dict>
</dict>
</plist>
"""
    return doc


def main():
    desks = sorted(d for d in AU.iterdir()
                   if d.is_dir() and d.name.endswith("AudioUnit")
                   and not d.name.endswith("iOSAudioUnit") and d.name not in SKIP)
    # EQAudioUnit (macOS) maps to the existing EQiOSAudioUnit: skip it too.
    desks = [d for d in desks if d.name != "EQAudioUnit"]
    maps = [parse_desktop(d) for d in desks]

    subtypes = [m["subtype"] for m in maps]
    dupes = sorted({s for s in subtypes if subtypes.count(s) > 1})
    if dupes:
        sys.exit(f"DUPLICATE SUBTYPES: {dupes}")
    print(f"{len(maps)} plugins, all subtypes unique")
    for m in maps:
        print(f"  {m['base']:22s} type={m['plugin_type']!r:24s} subtype={m['subtype']} tags={m['tags']}")

    if "--generate" not in sys.argv:
        print("dry run; pass --generate to write files")
        return

    yml_targets = []
    yml_embeds = []
    text = YML.read_text()
    for m in maps:
        target = f"{m['base']}iOSAudioUnit"
        unit_class = target
        vc_class = f"{m['base']}iOSViewController"
        out = AU / target
        out.mkdir(exist_ok=True)
        write_if_different(out / f"{unit_class}.swift", UNIT_SWIFT.format(
            unit_class=unit_class, plugin_type=m["plugin_type"],
            subtype=m["subtype"], plugin_name=m["plugin_name"]))
        write_if_different(out / f"{vc_class}.swift", VC_SWIFT.format(
            vc_class=vc_class, unit_class=unit_class, plugin_type=m["plugin_type"]))
        write_if_different(out / "Info.plist", plist_for(m, target))
        if f"\n  {target}:\n" in text:
            continue
        yml_targets.append(f"""
  {target}:
    templates:
      - SOTFiOSAudioUnitExtension
    sources:
      - path: {target}
        name: {target}
        createIntermediateGroups: true
    settings:
      PRODUCT_BUNDLE_IDENTIFIER: org.spinorama.SOTFAudioUnits.iOS.{m['base']}AudioUnit
      INFOPLIST_FILE: {target}/Info.plist
""")
        yml_embeds.append(f"""      - target: {target}
        embed: true
""")

    if not yml_targets and not yml_embeds:
        print(f"{len(maps)} wrappers up to date, project-ios.yml unchanged")
        return
    anchor = """    dependencies:
      - target: EQiOSAudioUnit
        embed: true
"""
    assert anchor in text, "app embed anchor not found"
    text = text.replace(anchor, anchor + "".join(yml_embeds), 1)
    text = text.rstrip("\n") + "\n" + "".join(yml_targets)
    if write_if_different(YML, text):
        print("project-ios.yml updated")
    print(f"wrote {len(maps)} wrappers, patched project-ios.yml")


main()
