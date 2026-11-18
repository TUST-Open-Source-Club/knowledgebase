use std::sync::atomic::AtomicU64;
use std::time::{SystemTime,UNIX_EPOCH};
use std::thread;


/// 占10位的worker_id,1023
const MAX_WORKER_ID: u16 =  (1 << 10) - 1;

pub struct Snowflake {
    worker_id: u16,
    }

impl Snowflake {
    pub fn Err_out_of_worker_id(worker_id: u16) -> Result<Self,String> {
        if worker_id > MAX_WORKER_ID {
            return Err("worker_id超限，最大允许1023".to_string());
        }
        Ok(Self { worker_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// 测试worker_id是否超限
    fn test_worker_id(){
        // max = 1023
        assert!(Snowflake::Err_out_of_worker_id(1024).is_err());
        assert!(Snowflake::Err_out_of_worker_id(1023).is_ok());
        // min = 0
        assert!(Snowflake::Err_out_of_worker_id(0).is_ok());
    }
}