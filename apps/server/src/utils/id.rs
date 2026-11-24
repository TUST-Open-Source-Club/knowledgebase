use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 2024-01-01 00:00:00 的毫秒时间戳
const EPOCH: u64 = 1_704_067_200_000;

const WORKER_ID_BITS: u64 = 10;
/// 占10位的worker_id,1023，接口要u16
const MAX_WORKER_ID: u16 = (1 << WORKER_ID_BITS) - 1;

const SEQUENCE_BITS: u64 = 12;
/// 占12位的sequence,4095
const MAX_SEQUENCE: u64 = (1 << SEQUENCE_BITS) - 1; // 4095

/// 左移量
const TIMESTAMP_SHIFT: u64 = WORKER_ID_BITS + SEQUENCE_BITS; // 22
const WORKER_ID_SHIFT: u64 = SEQUENCE_BITS; // 12

/// 最大睡眠时间，避免生成重复id
const MAX_SLEEP_MILLIS: u64 = 5;

pub struct Snowflake {
    worker_id: u16,
    state: AtomicU64,
}

/// 雪花ID生成器。
/// 
/// 位布局从高到底:
/// 1位恒为0的符号  | 41位时间戳 | 10位worker_id | 12位sequence序列号
impl Snowflake {
    // 获取当前时间戳，单位：ms
    fn current_timestamp_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间早于 UNIX_EPOCH")
            .as_millis() as u64
    }

    /// 拼装最终id
    fn compose_id(current_timestamp: u64, worker_id: u16, sequence: u64) -> i64 {
        let timestamp = (current_timestamp - EPOCH) << TIMESTAMP_SHIFT;
        let worker_part = (worker_id as u64) << WORKER_ID_SHIFT;
        let sequence_part = sequence & MAX_SEQUENCE;
        (timestamp | worker_part | sequence_part) as i64
    }

    /// 创建生成器
    /// 
    /// 若worker_id超限，则返回错误。
    pub fn new(worker_id: u16) -> Result<Self, String> {
        if worker_id > MAX_WORKER_ID {
            return Err("worker_id超限，最大允许1023".to_string());
        }
        Ok(Self {
            worker_id,
            state: AtomicU64::new(0),
        })
    }

    /// 生成下一个雪花ID。
    /// 
    /// 若时钟回拨，
    /// 则允许等待不超过最大回拨时间5ms，
    /// 超过则返回错误。
    pub fn next_id(&self) -> Result<i64, String> {
        loop {
            // 获取当前时间戳
            let now = Self::current_timestamp_millis();

            let current_state = self.state.load(Ordering::Relaxed);
            let last_ts = current_state >> SEQUENCE_BITS;
            let last_seq = current_state & MAX_SEQUENCE;

            // 若时钟回拨，
            // 设置最大回拨时间为MAX_SLEEP_MILLIS=5，
            // 超过则返回错误
            if now < last_ts {
                let sleep_time = last_ts - now;
                if sleep_time > MAX_SLEEP_MILLIS {
                    return Err(format!(
                        "时钟回拨超过{sleep_time}，超过允许的{MAX_SLEEP_MILLIS}"
                    ));
                }
                thread::sleep(Duration::from_millis(sleep_time));
                continue;
            }

            // 若时间戳相同，判断序列号是否超限
            let sequence = if now == last_ts {
                // 超限，等下一毫秒
                if last_seq >= MAX_SEQUENCE {
                    thread::sleep(Duration::from_millis(1));

                    continue;
                }
                // 未超限，增加序列号
                last_seq + 1
            } else {
                // 若时间戳进入新的毫秒，归零sequence
                0
            };

            let new_state = (now << SEQUENCE_BITS) | sequence;
            if self
                .state
                .compare_exchange(
                    current_state,
                    new_state,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return Ok(Self::compose_id(now, self.worker_id, sequence));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    #[test]
    /// 测试worker_id是否超限
    fn test_worker_id() {
        // max = 1023
        assert!(Snowflake::new(1024).is_err());
        assert!(Snowflake::new(1023).is_ok());
        // min = 0
        assert!(Snowflake::new(0).is_ok());
    }

    #[test]
    /// 测试拼装id,这段是问ai他建议的。检查序列号是否在最右侧
    fn test_compose_id() {
        let a = Snowflake::compose_id(EPOCH + 1, 0, 0);
        let b = Snowflake::compose_id(EPOCH + 1, 0, 1);
        assert_eq!(b - a, 1);
    }

    #[test]
    /// 一万次生成无重复
    fn test_generate_id_no_repeat() {
        let generator = Snowflake::new(1).expect("合法worker_id创建失败");
        // 用一万位hashset，检验重复id
        let mut set = HashSet::with_capacity(10_000);
        for _ in 0..10_000 {
            let id = generator.next_id().expect("生成id失败");
            assert!(set.insert(id), "出现重复 ID: {id}");
        }
        assert_eq!(set.len(), 10_000);
    }

    #[test]
    /// 同一毫秒内生成id递增
    fn test_generate_id_no_repeat_multi_thread() {
        let generator = Snowflake::new(2).expect("合法worker_id创建失败");
        let mut prev_id = generator.next_id().expect("生成id失败");
        let mut same_ms = 0;

        for _ in 0..1_000 {
            let id = generator.next_id().expect("生成id失败");
            let shift = TIMESTAMP_SHIFT;
            let prev_ts = (prev_id as u64) >> shift;
            let curr_ts = (id as u64) >> shift;
            if prev_ts == curr_ts {
                assert!(id > prev_id, "同一毫秒内应递增：prev_id={prev_id}, id={id}");
                same_ms += 1;
            }
            prev_id = id;
        }
        assert!(same_ms > 0, "同一毫秒至少一对id");
    }

    #[test]
    /// 多线程并发不重复
    fn test_generate_id_no_concurrent() {
        let generator = Arc::new(Snowflake::new(3).expect("合法worker_id创建失败"));

        // 8个线程，每个线程生成1000个id
        let test_gen_count = 8;
        let per_count = 1_000;
        // 建个列表放线程回执
        let mut handles = Vec::with_capacity(test_gen_count);
        for _ in 0..test_gen_count {
            let generator = Arc::clone(&generator);
            // spawn一个新线程，并把回执放进列表，8次
            handles.push(thread::spawn(move || {
                // 建个列表放id
                let mut ids = Vec::with_capacity(per_count);

                // 生成1000个id
                for _ in 0..per_count {
                    ids.push(generator.next_id().expect("生成id失败"));
                }

                ids
            }));
        }
        // 建个集合放所有id
        let mut all_ids = HashSet::new();
        for handle in handles {
            // 等线程结束，把id放进集合
            for id in handle.join().expect("线程join失败") {
                assert!(all_ids.insert(id));
                // 集合长度应该等于8000
            }
        }
        assert_eq!(all_ids.len(), test_gen_count * per_count);
    }
}
