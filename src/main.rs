use std::{
    collections::HashMap,
    env::{self, args},
    ffi::OsString,
    fs::{self, File},
    io::{stdin, Read, BufRead, BufReader},
    path::PathBuf,
    process::exit,
};

use baidu_fanyi::{
    mini_fmt::Fmtter,
    traits::FilterOutLongEmpty
};
use either::Either::{Left, Right};
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
pub const MAX_REQUEST_BYTES: usize = 3000;


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
#[inline]
fn help(opts: &getopts::Options, code: i32) -> ! {
    macro_rules! concatn {
        ( $( $line:expr ),* $(,)? ) => {
            concat!( $( $line, "\n" ),* )
        };
    }
    let bin_name = env!("CARGO_BIN_NAME");
    let biref = opts.short_usage(bin_name);
    let option = opts.usage(&format!("{biref} <FILE>"));
    let cfg = config_path();
    eprint!(concatn!{
        "{option}",
        "NOTE:",
        "    <FILE> is - use stdin",
        "    config file in {cfg:?},",
        "        line1: appid, line2: appkey",
        "",
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
    }, option=option, cfg=cfg);
    exit(code);
}

fn get_cfg() -> Config {
    let args = args().collect::<Vec<_>>();

    let mut opts = getopts::Options::new();

    macro_rules! decl {
        (@str $t:literal) => ($t);
        (@str $t:tt) => (stringify!($t));

        (@arg $t:literal) => (concat!("<", $t, ">"));
        (@arg $t:tt) => (concat!("<", stringify!($t), ">"));

        (-$short:ident --$long:tt * $desc:literal) => {
            opts.optflagmulti(stringify!($short), decl!(@str $long), $desc);
        };

        (-$short:ident --$long:tt $desc:literal) => {
            opts.optflag(stringify!($short), decl!(@str $long), $desc);
        };

        (-$short:ident --$long:tt (*$hint:tt) $desc:literal) => {
            opts.optmulti(
                stringify!($short),
                decl!(@str $long),
                $desc,
                decl!(@arg $hint),
            );
        };

        (-$short:ident --$long:tt ($hint:tt) $desc:literal) => {
            opts.optopt(
                stringify!($short),
                decl!(@str $long),
                $desc,
                decl!(@arg $hint),
            );
        };
    }

    opts.parsing_style(getopts::ParsingStyle::StopAtFirstFree);

    decl!(-f --from (lang)              "from lang");
    decl!(-t --to (lang)                "to lang");
    decl!(-l --line                     "read one line");
    decl!(-m --fmt (*fstr)              "formatters (multiple)");
    decl!(-o --"empty-count" (count)    "filter out empty count (default:2)");
    decl!(-v --version*                 "show version");
    decl!(-h --help*                    "show help");

    let parsed = match opts.parse(&args[1..]) {
        Ok(parsed) => parsed,
        Err(getopts::Fail::ArgumentMissing(opt)) => {
            eprintln!("Error: argument missing {opt}");
            help(&opts, 2);
        },
        Err(getopts::Fail::UnrecognizedOption(opt)) => {
            eprintln!("Error: invalid option {opt}");
            help(&opts, 2);
        },
        Err(getopts::Fail::OptionMissing(opt)) => {
            eprintln!("Error: missing required option {opt}");
            help(&opts, 2);
        },
        Err(getopts::Fail::OptionDuplicated(opt)) => {
            eprintln!("Error: option duplicated {opt}");
            help(&opts, 2);
        },
        Err(getopts::Fail::UnexpectedArgument(opt)) => {
            eprintln!("Error: unexpected argument {opt}");
            help(&opts, 2);
        },
    };

    if parsed.opt_present("help") { help(&opts, 0) }
    if parsed.opt_present("version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        exit(0)
    }

    let mut cfg = Config::default();

    cfg.long_empty_count = parsed.opt_get_default("o", 2)
        .unwrap_or_else(|e| {
            eprintln!("Error: parse to int error `{}`", e);
            help(&opts, 2)
        });
    cfg.from_lang = parsed.opt_str("from");
    cfg.to_lang = parsed.opt_str("to");

    let mut fmtters = parsed.opt_strs("m");
    if fmtters.is_empty() { fmtters.push(DEFAULT_OUT_FORMAT.to_owned()) }
    for formatter in fmtters {
        match formatter.parse() {
            Ok(format) => cfg.format.push(format),
            Err(e) => {
                eprintln!("Error: on `{formatter}` build fmtter error: {e}");
                help(&opts, 2)
            },
        }
    }

    let filename = match &parsed.free[..] {
        [name] => name,
        [] => {
            eprintln!("Error: free argument missing");
            help(&opts, 2);
        }
        [_, args @ ..] => {
            eprintln!("Error: unexpected free arguments {args:?}");
            help(&opts, 2);
        }
    };

    let mut reader = match &**filename {
        "-" => Left(stdin().lock()),
        path => {
            Right(BufReader::new(File::open(path).unwrap_or_else(|e| {
                eprintln!("Error: open file error `{e}`");
                exit(3)
            })))
        },
    };

    cfg.text.clear();
    let err = if parsed.opt_present("line") {
        reader.read_line(&mut cfg.text)
    } else {
        reader.read_to_string(&mut cfg.text)
    };
    if let Err(e) = err {
        eprintln!("Error: read text error `{e}`");
        exit(3)
    };
    cfg.text = (&*cfg.text).filter_out_long_empty(cfg.long_empty_count);

    cfg
}


/// 格式化返回的 json 数据
#[inline]
fn format_out(fmtters: &Vec<Fmtter>, object: JSONData) -> Result<Vec<String>, String> {
    if let Some(lines) = object.get("trans_result") {
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
        let mut res_lines: Vec<String>
            = Vec::with_capacity(strs.len() * fmtters.len());
        for fmtter in fmtters.iter() {
            for item in strs.iter() {
                res_lines.push(fmtter.fmt_str(item))
            }
        }
        Ok(res_lines)
    } else {
        Err(format!("result data error: {:#?}", object))
    }
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
    match format_out(&cfg.format, result) {
        Ok(msg) => {
            for line in msg {
                print!("{}", line)
            }
        }
        Err(e) => panic!("{}", e),
    }
}
