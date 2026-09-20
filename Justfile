# --------------------------------------------------------- -*- just -*-
# sotf-daw: DAW core workspace (engine, plugins, MIDI, IAMF, drivers).
#
# Invoke from this directory so recipes resolve against sotf-daw/Cargo.toml.
# ----------------------------------------------------------------------

_default:
	just --list

import 'crates/sotf-plugins/Justfile'
import 'crates/sotf-engine/Justfile'
