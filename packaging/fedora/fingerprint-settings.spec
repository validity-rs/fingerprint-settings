# Fedora package of Fingerprints (fingerprint-settings), built from the
# vendored source tarball (ci/packages/tarball.sh) with Fedora's Rust and
# Fedora's Rust macros. CI: ci/packages/fedora.sh (rpmbuild, rpmlint), then
# ci/packages/install.sh (install, launch, remove).

%global appid io.github.validity_rs.FingerprintSettings

Name:           fingerprint-settings
Version:        0.1.1
Release:        1%{?dist}
Summary:        Manage your own fingerprints with any fprintd-compatible service

SourceLicense:  MIT
# The licenses of the crates compiled into the binary, as printed by
# %%{cargo_license_summary} (ci/packages/fedora.sh fails when they change):
# BEGIN LICENSE SUMMARY
# (MIT OR Apache-2.0) AND Unicode-3.0
# Apache-2.0 OR MIT
# Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT
# MIT
# MIT OR Apache-2.0
# MIT OR Apache-2.0 OR LGPL-2.1-or-later
# Unlicense OR MIT
# END LICENSE SUMMARY
License:        MIT AND Unicode-3.0 AND (Apache-2.0 OR MIT) AND (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND (MIT OR Apache-2.0 OR LGPL-2.1-or-later) AND (Unlicense OR MIT)
URL:            https://github.com/validity-rs/fingerprint-settings
Source0:        %{url}/releases/download/v%{version}/%{name}-%{version}.tar.gz

ExclusiveArch:  %{rust_arches}

BuildRequires:  cargo-rpm-macros >= 26
BuildRequires:  rust >= 1.99
BuildRequires:  pkgconfig(gtk4) >= 4.18
BuildRequires:  pkgconfig(libadwaita-1) >= 1.7
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

Requires:       hicolor-icon-theme
# Any service that speaks fprintd's D-Bus API; validity-rs's daemon is one.
Requires:       (fprintd or validity-daemon)

%description
A small desktop application to register your right index finger, or remove
it, in one plain screen. Made for desktops without fingerprint
settings of their own, such as Xfce, MATE and LXQt. It works with any
fingerprint service that speaks the fprintd D-Bus interface, fprintd itself
included. Your password always keeps working.

%prep
%autosetup -n %{name}-%{version}
# The tarball's vendor/ directory: no crate is downloaded.
%cargo_prep -v vendor

%build
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%{cargo_vendor_manifest}

%install
install -Dpm 0755 target/rpm/%{name} %{buildroot}%{_bindir}/%{name}
install -Dpm 0644 packaging/common/%{appid}.desktop %{buildroot}%{_datadir}/applications/%{appid}.desktop
install -Dpm 0644 packaging/common/%{appid}.metainfo.xml %{buildroot}%{_metainfodir}/%{appid}.metainfo.xml
install -Dpm 0644 packaging/common/icons/hicolor/scalable/apps/%{appid}.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{appid}.svg
install -Dpm 0644 packaging/common/%{name}.1 %{buildroot}%{_mandir}/man1/%{name}.1

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{appid}.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/%{appid}.metainfo.xml
%cargo_test

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/%{appid}.desktop
%{_metainfodir}/%{appid}.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/%{appid}.svg
%{_mandir}/man1/%{name}.1*

%changelog
* Sun Oct 25 2026 pierre-warnier <409744+pierre-warnier@users.noreply.github.com> - 0.1.1-1
- One screen for the right index finger, a progress bar, plain instructions.

* Fri Oct 23 2026 pierre-warnier <409744+pierre-warnier@users.noreply.github.com> - 0.1.0-1
- First release: add, name, test, rename and remove your own fingerprints
  with any fprintd-compatible service.
