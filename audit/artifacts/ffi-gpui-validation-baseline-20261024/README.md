# Isolated coherent FFI dependency baseline

The ordinary DAW FFI build failed before feature testing with 69 duplicate-GPUI
type errors. Its lock combines toolkit commit `483ea7c7` (using GPUI branch
commit `4cbf3281`) with direct GPUI commit `1fe02c0b`. Unchanged GUI code consequently
uses incompatible copies of nominally identical types.

An isolated manifest tree retained the actual current source through symlinks.
Its lock was aligned with the existing coherent sibling app family: toolkit
`f556d489c6b78ff3c41d1b858ad44f76b59d2916` and GPUI
`d476ae9bc3165145d4a8e43b6bafbef11b196a04`. Necessary transitive changes and removal
of the obsolete branch copy are preserved in the 3304-line fixture diff. These
are validation-only dependency changes; the main manifests and working lock
were not upgraded to this family.

Against that fixture, all fifteen targeted FFI tests and normal-profile strict
Clippy pass. Missing existing-lock cache packages were fetched without changing
either lock: `plist 1.10.3` and `webpki-root-certs 1.0.11`. The source owner records
exact commands/hashes and outcomes under
`../remaining-linear-eq-accuracy-20261024/`.

The ordinary-main dependency failure remains open. Release-profile strict FFI
Clippy also retains three preexisting literal-unwrap findings in unchanged
processing branches; the normal-profile strict check does not erase them. This
fixture is not an ordinary-main or cross-platform release certification.
