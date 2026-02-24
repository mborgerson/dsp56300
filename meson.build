project('dsp56300', version: '0.1.0')

dsp56300_src = meson.current_source_dir()

dsp56300_lib = custom_target(
  'dsp56300_emu_ffi',
  command: [
    'sh', '-c',
    'cargo build --release -p dsp56300-emu-ffi --no-default-features --manifest-path "$1/Cargo.toml" && cp "$1/target/release/libdsp56300_emu_ffi.a" "$2"',
    '--', dsp56300_src, '@OUTPUT@'
  ],
  output: 'libdsp56300_emu_ffi.a',
  build_by_default: true,
  build_always_stale: true,
  console: true,
)

dsp56300_dep = declare_dependency(
  sources: dsp56300_lib,
  link_args: [dsp56300_lib.full_path(), '-lpthread', '-ldl', '-lm'],
  include_directories: include_directories('crates/emu-ffi/include'),
)

meson.override_dependency('dsp56300-emu-ffi', dsp56300_dep)
