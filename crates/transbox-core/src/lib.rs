//! Tiny authenticated envelope around SOCKS5. DNS resolution happens in the relay.
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::thread;
use std::time::Duration;

pub const MAGIC: &[u8; 4] = b"TBX1";
pub const CONNECT: u8 = 1;
pub const HEARTBEAT: u8 = 2;
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub fn decode_token(value: &str) -> io::Result<[u8; 32]> {
    if value.len() != 64 || !value.is_ascii() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "token must be 64 hex characters",
        ));
    }
    let mut token = [0; 32];
    for (i, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid token"))?;
    }
    Ok(token)
}

pub fn authenticate(stream: &mut TcpStream, token: &[u8; 32], operation: u8) -> io::Result<()> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.write_all(MAGIC)?;
    stream.write_all(token)?;
    stream.write_all(&[operation])?;
    let mut result = [0];
    stream.read_exact(&mut result)?;
    if result[0] != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "relay authentication failed",
        ));
    }
    Ok(())
}

pub fn accept_auth(stream: &mut TcpStream, token: &[u8; 32]) -> io::Result<u8> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut header = [0; 37];
    stream.read_exact(&mut header)?;
    let difference = header[4..36]
        .iter()
        .zip(token)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if &header[..4] != MAGIC || difference != 0 || ![CONNECT, HEARTBEAT].contains(&header[36]) {
        let _ = stream.write_all(&[1]);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid relay envelope",
        ));
    }
    stream.write_all(&[0])?;
    Ok(header[36])
}

/// Preserve TCP half-closes (needed by HTTP and upload protocols).
pub fn bridge(mut left: TcpStream, mut right: TcpStream) -> io::Result<(u64, u64)> {
    for stream in [&left, &right] {
        // Windows accept() inherits its listener's nonblocking mode. The broker
        // polls a nonblocking listener but forwards each stream with blocking IO.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
    }
    let mut left_copy = left.try_clone()?;
    let mut right_copy = right.try_clone()?;
    let upload = thread::spawn(move || {
        let result = io::copy(&mut left_copy, &mut right_copy);
        let _ = right_copy.shutdown(Shutdown::Write);
        if result.is_err() {
            let _ = left_copy.shutdown(Shutdown::Both);
            let _ = right_copy.shutdown(Shutdown::Both);
        }
        result
    });
    let download = io::copy(&mut right, &mut left);
    let _ = left.shutdown(Shutdown::Write);
    if download.is_err() {
        let _ = left.shutdown(Shutdown::Both);
        let _ = right.shutdown(Shutdown::Both);
    }
    let uploaded = upload
        .join()
        .map_err(|_| io::Error::other("copy thread panicked"))??;
    Ok((uploaded, download?))
}

/// The relay is for public Internet access; never expose phone/LAN services to pages.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..=127).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0))
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v.segments();
            // Global unicast only. Exclude documentation, Teredo and 6to4 translation.
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] == 0 || s[1] == 0xdb8))
                && s[0] != 0x2002
        }
    }
}

fn reply(stream: &mut TcpStream, status: u8) -> io::Result<()> {
    stream.write_all(&[5, status, 0, 1, 0, 0, 0, 0, 0, 0])
}

pub fn serve_socks(mut client: TcpStream) -> io::Result<()> {
    let mut greeting = [0; 2];
    client.read_exact(&mut greeting)?;
    if greeting[0] != 5 || greeting[1] == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SOCKS greeting",
        ));
    }
    let mut methods = vec![0; greeting[1] as usize];
    client.read_exact(&mut methods)?;
    if !methods.contains(&0) {
        client.write_all(&[5, 255])?;
        return Ok(());
    }
    client.write_all(&[5, 0])?;
    let mut request = [0; 4];
    client.read_exact(&mut request)?;
    if request[0] != 5 || request[2] != 0 {
        reply(&mut client, 1)?;
        return Ok(());
    }
    if request[1] != 1 {
        reply(&mut client, 7)?;
        return Ok(());
    }
    let host = match request[3] {
        1 => {
            let mut ip = [0; 4];
            client.read_exact(&mut ip)?;
            IpAddr::from(ip).to_string()
        }
        4 => {
            let mut ip = [0; 16];
            client.read_exact(&mut ip)?;
            IpAddr::from(ip).to_string()
        }
        3 => {
            let mut length = [0];
            client.read_exact(&mut length)?;
            let mut domain = vec![0; length[0] as usize];
            client.read_exact(&mut domain)?;
            match String::from_utf8(domain) {
                Ok(name) if !name.is_empty() && !name.contains('\0') => name,
                _ => {
                    reply(&mut client, 4)?;
                    return Ok(());
                }
            }
        }
        _ => {
            reply(&mut client, 8)?;
            return Ok(());
        }
    };
    let mut port = [0; 2];
    client.read_exact(&mut port)?;
    // This getaddrinfo call executes on Android, never on the desktop.
    let addresses: Vec<SocketAddr> =
        match (host.as_str(), u16::from_be_bytes(port)).to_socket_addrs() {
            Ok(addresses) => addresses.filter(|a| is_public(a.ip())).take(8).collect(),
            Err(_) => {
                reply(&mut client, 4)?;
                return Ok(());
            }
        };
    if addresses.is_empty() {
        reply(&mut client, 2)?;
        return Ok(());
    }
    for address in addresses {
        if let Ok(remote) = TcpStream::connect_timeout(&address, Duration::from_secs(4)) {
            remote.set_nodelay(true)?;
            reply(&mut client, 0)?;
            bridge(client, remote)?;
            return Ok(());
        }
    }
    reply(&mut client, 5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn denies_local_and_translation_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.1.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "2606:4700:4700::1111",
            "::ffff:1.1.1.1",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn rejects_bad_authentication() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(accept_auth(&mut stream, &[1; 32]).is_err());
        });
        let mut stream = TcpStream::connect(address).unwrap();
        assert!(authenticate(&mut stream, &[2; 32], HEARTBEAT).is_err());
        server.join().unwrap();
    }

    #[test]
    fn half_close_still_delivers_response() {
        let ingress = TcpListener::bind("127.0.0.1:0").unwrap();
        let egress = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut caller = TcpStream::connect(ingress.local_addr().unwrap()).unwrap();
        caller
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let upstream = TcpStream::connect(egress.local_addr().unwrap()).unwrap();
        let (mut destination, _) = egress.accept().unwrap();
        let accepted = ingress.accept().unwrap().0;
        accepted.set_nonblocking(true).unwrap();
        upstream.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || bridge(accepted, upstream).unwrap());
        thread::sleep(Duration::from_millis(50));
        caller.write_all(b"request").unwrap();
        caller.shutdown(Shutdown::Write).unwrap();
        let mut request = Vec::new();
        destination.read_to_end(&mut request).unwrap();
        assert_eq!(request, b"request");
        destination.write_all(b"response").unwrap();
        destination.shutdown(Shutdown::Write).unwrap();
        let mut response = Vec::new();
        caller.read_to_end(&mut response).unwrap();
        assert_eq!(response, b"response");
        assert_eq!(worker.join().unwrap(), (7, 8));
    }
}
