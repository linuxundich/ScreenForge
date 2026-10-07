project(
  'screenforge',
  'rust',
  version: '0.34.1',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.0',
)

i18n = import('i18n')
gnome = import('gnome')

app_id = 'de.christophlangner.ScreenForge'
prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
localedir = prefix / get_option('localedir')

dependency('gtk4', version: '>= 4.22')
dependency('libadwaita-1', version: '>= 1.9')

subdir('app/data')
subdir('po')

# The binary itself is built by cargo; meson only passes the install
# paths in (compiled into the binary via option_env!).
cargo_build = find_program('build-aux/cargo-build.sh')
custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  console: true,
  output: 'screenforge',
  install: true,
  install_dir: bindir,
  command: [
    cargo_build,
    meson.project_source_root(),
    meson.project_build_root(),
    localedir,
    get_option('buildtype'),
    '@OUTPUT@',
  ],
)

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
