use std::sync::atomic::AtomicU64;
use std::time::{SystemTime,UNIX_EPOCH,Duration};
use std::thread;





/// 2024-01-01 00:00:00 的毫秒时间戳
const EPOCH: u64 = 1_704_067_200_000;

const WORKER_ID_BITS: u64 = 10;
/// 占10位的worker_id,1023，接口要u16
const MAX_WORKER_ID: u16 =  (1 << WORKER_ID_BITS) - 1;

const SEQUENCE_BITS: u64 = 12;
/// 占12位的sequence,4095
const MAX_SEQUENCE: u64 = (1 << SEQUENCE_BITS) - 1;  // 4095

///左移量
const TIMESTAMP_SHIFT: u64 = WORKER_ID_BITS + MAX_SEQUENCE; // 22
const WORKER_ID_SHIFT: u64 = MAX_SEQUENCE; // 12

/// 最大睡眠时间，避免生成重复id
const MAX_SLEEP_MILLIS: u64 = 5;



pub struct Snowflake {
    worker_id: u16,
    last_ts: u64,
    sequence: u64,
    }

impl Snowflake {

    /// 获取当前时间戳，单位：ms
    fn current_timestamp_millis() -> u64 {
        use std::time::{SystemTime,UNIX_EPOCH};
        SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间早于 UNIX_EPOCH")
        .as_millis() as u64
    }


    /// 拼装最终id
    fn compose_id(current_timestamp: u64, worker_id: u16, sequence: u64) -> i64 {
        let timestamp = (current_timestamp - EPOCH) << TIMESTAMP_SHIFT;
        let worker_part = (worker_id as u64) << 
        WORKER_ID_SHIFT;
        let sequence_part = sequence & MAX_SEQUENCE;
        (timestamp | worker_part | sequence_part) as i64
        }

    /// 判断worker_id是否超限，并构造生成器
    pub fn new(worker_id: u16) -> Result<Self,String> {
        if worker_id > MAX_WORKER_ID {
            return Err("worker_id超限，最大允许1023".to_string());
        }
        Ok(Self { worker_id,
                  last_ts: 0,
                  sequence: 0 })
    }

    /// 生成下一个雪花ID
    pub fn next_id(&mut self) -> Result<i64,String>{
        loop {
            // 获取当前时间戳
            let now = Self::current_timestamp_millis();
            
            // 若时钟回拨，
            // 设置最大回拨时间为MAX_SLEEP_MILLIS=5，
            // 超过则返回错误
            if now < self.last_ts {
                let sleep_time = self.last_ts - now;
                if sleep_time > MAX_SLEEP_MILLIS {
                    return Err("时钟回拨超过{sleep_time}，超过允许的{MAX_SLEEP_MILLIS}".to_string());
                }
                thread::sleep(Duration::from_millis(sleep_time));
                continue;
            }

            // 若时间戳相同，判断序列号是否超限
            if now == self.last_ts {
                // 超限，等下一毫秒
                if self.sequence >= MAX_SEQUENCE {
                    thread::sleep(Duration::from_millis(1));

                    continue;
                }
                // 未超限，增加序列号
                self.sequence += 1;
            }

            // 若时间戳进入新的毫秒，更新last_ts，归零sequence
            else {
                self.sequence = 0;
                self.last_ts = now;
            }

            return Ok(Self::compose_id(self.last_ts,
                self.worker_id,
                self.sequence))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// 测试worker_id是否超限
    fn test_worker_id(){
        // max = 1023
        assert!(Snowflake::new(1024).is_err());
        assert!(Snowflake::new(1023).is_ok());
        // min = 0
        assert!(Snowflake::new(0).is_ok());
    }


    #[test]
    /// 测试拼装id,这段是问ai他建议的。检查序列号是否在最右侧
    fn test_compose_id(){
        let a = Snowflake::compose_id(EPOCH + 1, 0, 0);
        let b = Snowflake::compose_id(EPOCH + 1, 0, 1);
        assert_eq!(b - a, 1);
    }
}

