//! Mock implementations of the backend traits for testing.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;

use super::{BridgeGuard, CreateBridgeParams, Device, InstanceConfig};

/// A mock [`super::InstanceBackend`] that records all calls for verification.
pub struct MockInstanceBackend {
    pub calls: Mutex<Vec<MockInstanceCall>>,
}

impl MockInstanceBackend {
    pub fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()) }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MockInstanceCall {
    Create { name: String },
    Start { name: String },
    Delete { name: String },
    AddDevice { name: String, dev_name: String },
    AttachToBridge { name: String, bridge: String },
    SetDescription { name: String },
    Exec { name: String },
    ExecStdout { name: String },
    WriteFile { name: String, path: String },
}

#[derive(Debug, thiserror::Error)]
#[error("mock error")]
pub struct MockError;

impl super::InstanceBackend for MockInstanceBackend {
    type Error = MockError;

    fn create(&self, name: &str, _config: &InstanceConfig) -> Result<(), Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::Create { name: name.to_string() });
        Ok(())
    }

    fn start(&self, name: &str) -> Result<(), Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::Start { name: name.to_string() });
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::Delete { name: name.to_string() });
        Ok(())
    }

    fn add_device(&self, name: &str, dev_name: &str, _device: &Device) -> Result<(), Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::AddDevice {
            name: name.to_string(),
            dev_name: dev_name.to_string(),
        });
        Ok(())
    }

    fn attach_to_bridge(&self, name: &str, bridge: &str, _ingress: Option<&str>, _egress: Option<&str>) -> Result<(), Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::AttachToBridge {
            name: name.to_string(),
            bridge: bridge.to_string(),
        });
        Ok(())
    }

    fn set_description(&self, name: &str, _desc: &str) -> Result<(), Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::SetDescription { name: name.to_string() });
        Ok(())
    }

    fn exec(&self, name: &str, _cmd: &[String], _env: &HashMap<String, String>, _cwd: &Path, _uid: u32, _gid: u32, _home: Option<&Path>, _proxy_url: Option<&str>) -> Result<i32, Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::Exec { name: name.to_string() });
        Ok(0)
    }

    fn exec_stdout(&self, name: &str, _cmd: &[&str]) -> Result<String, Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::ExecStdout { name: name.to_string() });
        Ok("running".to_string())
    }

    fn write_file(&self, name: &str, _content: &[u8], path: &str, _mode: &str, _uid: u32, _gid: u32) -> Result<(), Self::Error> {
        self.calls.lock().unwrap().push(MockInstanceCall::WriteFile {
            name: name.to_string(),
            path: path.to_string(),
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Mock network backend
// ---------------------------------------------------------------------------

pub struct MockNetworkBackend {
    pub calls: Mutex<Vec<MockNetworkCall>>,
}

impl MockNetworkBackend {
    pub fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()) }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MockNetworkCall {
    CreateBridge { name: String },
}

pub struct MockBridgeGuard {
    pub dropped: Mutex<bool>,
}

impl MockBridgeGuard {
    fn new() -> Self {
        Self { dropped: Mutex::new(false) }
    }
}

impl BridgeGuard for MockBridgeGuard {}

impl Drop for MockBridgeGuard {
    fn drop(&mut self) {
        *self.dropped.lock().unwrap() = true;
    }
}

impl super::NetworkBackend for MockNetworkBackend {
    type Error = MockError;

    fn create_bridge(&self, name: &str, _params: &CreateBridgeParams) -> Result<(Box<dyn BridgeGuard>, IpAddr), Self::Error> {
        self.calls.lock().unwrap().push(MockNetworkCall::CreateBridge { name: name.to_string() });
        let guard: Box<dyn BridgeGuard> = Box::new(MockBridgeGuard::new());
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        Ok((guard, ip))
    }
}
