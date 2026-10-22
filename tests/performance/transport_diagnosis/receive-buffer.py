from pathlib import Path
import shutil,subprocess
p=Path('/private/tmp/lra-path-diagnosis');f=p/'source/crates/network/src/service.rs';s=f.read_text();anchor='let endpoint = quinn::Endpoint::server(identity.server()?, config.listen)?;';assert s.count(anchor)==1
(p/'service-before-buffer.rs').write_text(s);shutil.copy2(p/'target/release/paneferry-network',p/'network-baseline')
s=s.replace(anchor,'''use std::os::fd::AsRawFd;
        let socket = std::net::UdpSocket::bind(config.listen)?;
        let receive_bytes: libc::c_int = 4 * 1024 * 1024;
        let result = unsafe { libc::setsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVBUF,
            &receive_bytes as *const _ as *const libc::c_void, std::mem::size_of_val(&receive_bytes) as libc::socklen_t) };
        ensure!(result == 0, "receive buffer configuration failed");
        let mut actual: libc::c_int = 0;
        let mut len = std::mem::size_of_val(&actual) as libc::socklen_t;
        let result = unsafe { libc::getsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVBUF,
            &mut actual as *mut _ as *mut libc::c_void, &mut len) };
        ensure!(result == 0, "receive buffer query failed");
        eprintln!("probe: SO_RCVBUF={actual}");
        let endpoint = quinn::Endpoint::new(Default::default(), Some(identity.server()?), socket, Arc::new(quinn::TokioRuntime))?;''');f.write_text(s)
subprocess.run(['cargo','build','--release','--offline','--locked','--manifest-path',str(p/'source/Cargo.toml'),'-p','paneferry-network'],env=__import__('os').environ|{'CARGO_TARGET_DIR':str(p/'target')},check=True)
