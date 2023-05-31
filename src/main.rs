use std::{
    collections::HashMap,
    env::{
        self, 
        args
    },
    ffi::OsString,
    fs::{
        self, 
        read_to_string
    },
    io::{
        stdin, 
        Read
    },
    path::PathBuf,
    process::exit,
};

use baidu_fanyi::{mini_fmt::Fmtter, traits::FilterOutLongEmpty};
use reqwest::{
    header::HeaderMap,
    Client,
    Error as RError,
    Response,
};
use serde_json::Value;
use lazy_static::lazy_static;
use md5::{
    self,
    Digest
};
use rand::random;


macro_rules! literals {
    ( $(
            #define $name:ident = $value:expr;
    )* ) => {
        $(
            macro_rules! $name {
                () => {
                    $value
                }
            }
        )*
    };
}
literals!{
    #define API_END_POINT = "http://api.fanyi.baidu.com";
    #define API_INTERFACE_PATH = "/api/trans/vip/translate";
}


pub const URL: &str = concat!(API_END_POINT!(), API_INTERFACE_PATH!());
pub const DEFAULT_FROM_LANG: &str = "auto";
pub const DEFAULT_TO_LANG: &str = "auto";
pub const MAX_TIMEOUT_COUNT: u32 = 2;
pub const MAX_ERROR_COUNT: u32 = 2;
pub const MAX_REQUEST_BYTES: usize = 4000;


lazy_static!{
    static ref HEADERS: HeaderMap = {
        let mut x = HeaderMap::new();
        // 'Content-Type': 'application/x-www-form-urlencoded'
        x.insert("Content-Type",
                 "application/x-www-form-urlencoded".parse().unwrap());
        x
    };
}


/// 传入累计大小
/// 修改大小计数并且返回是否需要分配新的一个块
pub fn split_blocks(sum: &mut usize, this: usize) -> Result<bool, ()> {
    if this < MAX_REQUEST_BYTES {
        let num = *sum + this;
        Ok(if num < MAX_REQUEST_BYTES {
            false
        } else {
            // 旧子块加上新子块超出了最大块大小
            // 将统计大小赋值为新块大小并通知新建块
            // 比较的子块将被放入新块
            *sum = this;
            true
        })
    } else {
        // 大于最大请求大小
        Err(())
    }
}


type JSONData = HashMap<String, Value>;


/// 构建 md5 值, 官方示例是 utf-8 编码, 而 rust 字符串为 utf-8, 因此不用转换
fn make_md5(s: &str) -> Digest {
    md5::compute(s.as_bytes())
}


/// 获取盐值
/// 官方要求盐值在 [32768,65536], 实在是阴间
/// 我推测可能是 [32768,65536) 因此使用我推测的值
fn get_salt() -> u16 {
    const MASK: u16 = 32767;
    const BASE: u16 = MASK + 1;
    let rand_num: u16 = random();
    let final_num = (rand_num & MASK) + BASE;
    final_num
}


async fn post(
    url: &str,
    headers: HeaderMap,
    data: &JSONData
    ) -> Result<Response, RError> {
    let client = Client::new();
    client.post(url)
        .headers(headers)
        .form(data)
        .send().await
}


fn get_id_and_key() -> [String; 2] {
    let path = config_path();
    let file: String = fs::read_to_string(&path)
        .unwrap_or_else(|e| {
            eprintln!("read config file error. path: {:?}, msg: {:?}",
                      &path, e.to_string());
            panic!();
        });
    let mut lines = file.lines();
    let msg: &str = "config lines < 2";
    [lines.next().expect(msg).into(), lines.next().expect(msg).into()]
}

fn config_path() -> OsString {
    let mut path = PathBuf::new();
    path.push(&env::var("HOME").expect("get home error")[..]);
    path.push(".baidufanyi_key");
    path.into()
}


#[derive(Clone, Copy)]
struct Translater<'a> {
    id: &'a str,
    key: &'a str,
    salt: u16,
    from_lang: &'a str,
    to_lang: &'a str,
}
impl<'a> Translater<'a> {
    pub fn new(id: &'a str, key: &'a str) -> Self {
        Self {
            id,
            key,
            salt: 0,
            from_lang: DEFAULT_FROM_LANG,
            to_lang: DEFAULT_TO_LANG,
        }
    }

    pub fn set_from_lang(&mut self, from: &'a str) -> &Self {
        self.from_lang = from;
        self
    }

    pub fn set_to_lang(&mut self, to: &'a str) -> &Self {
        self.to_lang = to;
        self
    }

    /// 更新盐值
    pub fn update_salt(&mut self) {
        self.salt = get_salt()
    }

    /// 构建请求荷载
    /// 'appid': appid,
    /// 'q': query,
    /// 'from': from_lang,
    /// 'to': to_lang,
    /// 'salt': salt,
    /// 'sign': sign
    pub fn build_payload(&self, message: String) -> JSONData {
        const KEY_COUNT: usize = 6;
        let sign = self.get_sign(&message); // 初始化签名
        let mut data = JSONData::with_capacity(KEY_COUNT);
        debug_assert!(data.capacity() >= KEY_COUNT); // 可能分配更多

        data.insert("appid".to_string(), self.id.into());
        data.insert("q".into(), message.into());
        data.insert("from".into(), self.from_lang.into());
        data.insert("to".into(), self.to_lang.into());
        data.insert("salt".into(), self.salt.into());
        data.insert("sign".into(), sign.into());
        data
    }

    /// 请求翻译
    /// 复制一份 Translater 进行配置获取
    pub async fn translate(mut self, message: String) -> JSONData {
        self.update_salt(); // 需要先初始化盐值
        let payload: JSONData = self.build_payload(message);
        let mut timeout_count: u32 = 0;
        let mut error_count: u32 = 0;
        let result = loop {
            match post(URL, HEADERS.clone(), &payload).await {
                Ok(val) => break val,
                Err(e) => {
                    if e.is_timeout() {
                        timeout_count += 1
                    } else {
                        error_count += 1
                    }
                    if timeout_count >= MAX_TIMEOUT_COUNT {
                        panic!("timeout count >= {}", MAX_TIMEOUT_COUNT)
                    }
                    if error_count >= MAX_ERROR_COUNT {
                        panic!("error count >= {}", MAX_ERROR_COUNT)
                    }
                }
            }
        };
        result.json::<JSONData>().await.expect("data to json error")
    }

    /// 构建 md5 签名, 官方示例组合方式为
    /// appid + query + salt + appkey
    /// salt 为一个 [32768,65536] 区间的整数字符串, 不进行定长
    pub fn get_sign(&self, message: &str) -> String {
        let strs: [&str; 4]
            = [&self.id, message, &self.salt.to_string(), &self.key];
        format!("{:x}", make_md5(&strs.concat()))
    }

    #[allow(unused)]
    pub fn from_lang(&self) -> &str {
        self.from_lang
    }

    #[allow(unused)]
    pub fn to_lang(&self) -> &str {
        self.to_lang
    }
}


const DEFAULT_OUT_FORMAT: &str = "%s\n%s\n";

struct Config {
    from_lang: Option<String>,
    to_lang: Option<String>,
    text: String,
    format: Vec<Fmtter>,
    long_empty_count: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self { 
            from_lang: None,
            to_lang: None,
            text: String::new(),
            format: vec![],
            long_empty_count: 2,
        }
    }
}

/// out help info and exit
fn help(code: i32) -> ! {
    macro_rules! concatn {
        ( $( $line:expr ),* $(,)? ) => {
            concat!( $( $line, "\n" ),* )
        };
    }
    eprint!(concatn!{
        "USAGE: {} [OPTIONS] <FILE>",
        "OPTIONS:",
        "    -f, --from       from lang",
        "    -t, --to         to lang",
        "    -m, --fmt        formatters (multiple)",
        "    -o               filter out empty count (default:2)",
        "    --               stop read options",
        "    -v, --version    version",
        "    -h, --help       help",
        "NOTE:",
        "    <FILE> is - use stdin",
        "    config file in {:?},",
        "        line1: appid, line2: appkey",
        "Format:",
        "    |----|-------------|",
        "    | %s | Display     |",
        "    | %r | Debug       |",
        "    | %R | DebugExpand |",
        "    | %n | LF          |",
        "    | %N | CR          |",
        "    | %t | Tab         |",
        "    | %e | ESC         |",
        "    | %x | ASCII       |",
        "    | %u | Unicode     |",
        "    | %U | Unicode+    |",
        "    |----|-------------|",
        "    `%[n]...` example: `%0s`, index 0 Display",
    }, env!("CARGO_BIN_NAME"), config_path());
    exit(code);
}


fn get_cfg() -> Config {
    let mut args = args();
    args.next().unwrap(); // self
    let mut cfg = Config::default();
    let mut with_args = false;
    let mut readed_file = false;
    macro_rules! get {
        ( $key:expr ) => {{
            args.next().unwrap_or_else(|| {
                eprintln!("key {} no value.", $key);
                exit(2);
            })
        }};
    }
    while let Some(i) = args.next() {
        macro_rules! get_file {
            ( $path:expr ) => {{
                let path = $path;
                if let Some(opt) = args.next() {
                    eprintln!("redundant options: {:?}", opt);
                    help(2);
                }
                if path == "-" {
                    // stdin
                    cfg.text.clear();
                    stdin().read_to_string(&mut cfg.text)
                        .unwrap_or_else(|e| {
                            eprintln!("readstdin error: {}", e);
                            exit(3);
                        });
                } else {
                    cfg.text = read_to_string(path)
                        .unwrap_or_else(|e| {
                            eprintln!("readfile error: {}", e);
                            exit(3);
                        });
                }
                readed_file = true;
                break;
            }};
        }
        with_args = true;
        match &*i {
            "-h" | "--help" => help(0),
            "-f" | "--from" => cfg.from_lang = Some(get!(i)),
            "-t" | "--to" => cfg.to_lang = Some(get!(i)),
            "-m" | "--fmt" => cfg.format.push(
                Fmtter::build(&get!(i))
                .unwrap_or_else(|e| {
                    eprintln!("build fmtter error: {}", e);
                    exit(2);
                })),
            "-o" => cfg.long_empty_count = get!(i).parse()
                .unwrap_or_else(|e| {
                    eprintln!("parse to int error: {}", e);
                    exit(2)
                }),
            "-v" | "--version" => {
                eprintln!("v{}", env!("CARGO_PKG_VERSION"));
                exit(0);
            },
            "--" => {
                get_file!(get!("FILE"))
            }
            path => {
                // file
                get_file!(path)
            }
        }
    }
    if cfg.format.len() == 0 {
        // use default formater
        cfg.format.push(Fmtter::build(DEFAULT_OUT_FORMAT).unwrap())
    }
    cfg.text = (&*cfg.text).filter_out_long_empty(cfg.long_empty_count);
    if ! with_args {
        eprintln!("error: no args");
        help(2);
    }
    if ! readed_file {
        eprintln!("error: no file");
        help(2);
    }
    cfg
}


#[tokio::main]
async fn main() {
    let cfg = get_cfg();
    let [id, key] = get_id_and_key();
    let mut translater = Translater::new(&id, &key);
    if let Some(x) = &cfg.from_lang {
        translater.set_from_lang(x);
    }
    if let Some(x) = &cfg.to_lang {
        translater.set_to_lang(x);
    }
    let result: JSONData
        = translater.translate(cfg.text).await;
    if let Some(lines) = result.get("trans_result") {
        let lines = lines.as_array().unwrap();
        let mut strs: Vec<[&str; 2]> = Vec::with_capacity(lines.len());
        for line in lines {
            let line = line.as_object().unwrap();
            strs.push([
                      line.get("dst").unwrap().as_str().unwrap(),
                      line.get("src").unwrap().as_str().unwrap()
            ]);
        }
        // formats
        for fmtter in cfg.format {
            for item in strs.iter() {
                print!("{}", fmtter.fmt_str(item))
            }
        }
    } else {
        eprintln!("result data error: {:#?}", result);
        exit(4);
    }
}
