Name: fscmp
Summary: Directory/file comparison utility
Version: %{_VERSION}
License: ASL 2.0
Release: 1

%description
%{summary}.

%build
cargo build --release --target %{_TARGET} %{?_CARGO_BUILD_ARGS}

%install
install -D -m 755 %{_sourcedir}/target/%{_TARGET}/release/%{name} -t %{buildroot}%{_bindir}

%files
%{_bindir}/*
