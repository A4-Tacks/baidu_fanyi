pub mod mini_fmt {
    use std::{fmt::{Debug, Display}, str::FromStr};

    #[derive(Debug, Clone, Copy)]
    pub enum FmtStyle {
        /// ToString (Display)
        Str,
        /// Repr (Debug)
        Repr,
        /// Expand repr (Debug)
        ERepr,
    }
    impl FmtStyle {
        pub fn fmt_str<S>(self, str: S) -> String
            where S: Debug + Display
        {
            match self {
                Self::Str => format!("{}", str),
                Self::Repr => format!("{:?}", str),
                Self::ERepr => format!("{:#?}", str),
            }
        }
    }
    #[derive(Debug, Clone)]
    pub enum FmtType {
        Const(String),
        Value { style: FmtStyle },
        IndexValue { id: usize, style: FmtStyle },
    }
    impl Default for FmtType {
        fn default() -> Self {
            Self::Const(String::default())
        }
    }
    impl FmtType {
        /// 格式化并移动格式化指针
        fn fmt_str<S>(&self, idx: &mut usize, args: &[S]) -> String
            where S: Display + Debug
        {
            use FmtType::*;
            let res = format!("{}", match self {
                Const(s) => s.into(),
                Value { style } => {
                    let tmp_idx = *idx;
                    *idx += 1;
                    style.fmt_str(&args[tmp_idx])
                },
                FmtType::IndexValue { id, style } => style.fmt_str(&args[*id]),
            });
            res
        }
    }
    /// 动态的格式化输入
    /// # Examples
    /// ```
    /// use baidu_fanyi::mini_fmt::Fmtter;
    /// let fmtter = Fmtter::build("ab%sde").unwrap();
    /// assert_eq!(&fmtter.fmt_str(&["c"]), "abcde");
    /// assert_eq!(&fmtter.fmt_str(&[7]), "ab7de");
    ///
    /// let fmtter = Fmtter::build("%s,%s,%0s,%1r,%s").unwrap();
    /// assert_eq!(&fmtter.fmt_str(&["a", "b", "c"]), "a,b,a,\"b\",c");
    ///
    /// assert_eq!(&Fmtter::build("%x1b").unwrap().fmt_str::<&str>(&[]), "\x1b");
    /// assert_eq!(&Fmtter::build("%x1C").unwrap().fmt_str::<&str>(&[]), "\x1c");
    /// assert_eq!(&Fmtter::build("%u0879").unwrap().fmt_str::<&str>(&[]), "\u{0879}");
    /// assert_eq!(&Fmtter::build("%U10ffff").unwrap().fmt_str::<&str>(&[]), "\u{10ffff}");
    /// assert!(Fmtter::build("%U110000").is_err());
    /// ```
    /// |----|-------------|
    /// | %s | Display     |
    /// | %r | Debug       |
    /// | %R | DebugExpand |
    /// | %n | LF          |
    /// | %N | CR          |
    /// | %t | Tab         |
    /// | %e | ESC         |
    /// | %x | ASCII       |
    /// | %u | Unicode     |
    /// | %U | Unicode+    |
    /// |----|-------------|
    ///
    /// `%[n]...` example: `%0s`, index 0 Display
    #[derive(Debug, Default)]
    pub struct Fmtter {
        args: Vec<FmtType>,
    }
    impl From<Vec<FmtType>> for Fmtter {
        fn from(args: Vec<FmtType>) -> Self {
            Self { args }
        }
    }
    impl TryFrom<&str> for Fmtter {
        type Error = String;
        fn try_from(value: &str) -> Result<Self, Self::Error> {
            Self::build(value)
        }
    }
    impl FromStr for Fmtter {
        type Err = String;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            s.try_into()
        }
    }
    impl Fmtter {
        /// new empty
        pub fn new() -> Self {
            Self::default()
        }
        /// from str build
        pub fn build(fmtter: &str) -> Result<Self, String> {
            let mut chars = fmtter.chars();
            let mut args: Vec<FmtType> = Vec::new();
            let mut last_val = String::new();
            while let Some(c) = chars.next() {
                macro_rules! add {
                    ( $val:expr ) => {{
                        if last_val.len() != 0 {
                            // 仅当前方有一个不为空的常量串时进行添加
                            args.push(FmtType::Const(last_val));
                            last_val = String::new();
                        }
                        args.push($val);
                    }};
                }
                macro_rules! res_seq_in_end {
                    () => {{
                        return Err("sequence in fmtter end".into());
                    }};
                }
                macro_rules! get_seq {
                    () => {{
                        if let Some(x) = chars.next() {
                            x
                        } else {
                            res_seq_in_end!()
                        }
                    }};
                }
                macro_rules! no_use {
                    ( ($( $a:tt )* ) [ $( $b:tt )* ]) => {
                        ($( $a )* )
                    };
                }
                macro_rules! add_hex {
                    ( ( $( $t:tt )* ) $type:tt ) => {{
                        let chars
                            = [$( no_use!((get_seq!())[$t]) ),*];
                        let hex = String::from_iter(chars);
                        if let Ok(val) = $type::from_str_radix(&hex, 16) {
                            last_val.push(
                                if let Some(x) = char::from_u32(val as u32) {
                                    x
                                } else {
                                    return Err(
                                        format!("{:x} to char failed", val))
                                })
                        } else {
                            return Err(format!("build hex error: {:?}", hex));
                        };
                    }};
                }
                /// 完成最终转义序列的匹配
                macro_rules! style_pat {
                    ( $val:expr ) => {{
                        match $val {
                            's' => FmtStyle::Str,
                            'r' => FmtStyle::Repr,
                            'R' => FmtStyle::ERepr,
                            x => return Err(
                                format!("unknown sequence: {:?}", x)),
                        }
                    }};
                }
                match c {
                    '%' => {
                        let next_c = get_seq!();
                        match next_c {
                            // 中间匹配或者截断
                            x @ '0'..='9' => {
                                // 元素位置引用 (没有支持10及以上的打算)
                                add!(FmtType::IndexValue {
                                    id: x.to_digit(10).unwrap() as usize,
                                    style: style_pat!(get_seq!())
                                })
                            },
                            '%' => last_val.push(c), // 普通的百分号
                            'n' => last_val.push('\n'), // 换行
                            'N' => last_val.push('\r'), // 回车
                            't' => last_val.push('\t'), // 制表
                            'e' => last_val.push('\x1b'), // ESC
                            'x' => add_hex!((++) u8), // ASCII
                            'u' => add_hex!((++++) u16), // Unicode
                            'U' => add_hex!((++++++) u32), // Unicode+
                            _ => add!(FmtType::Value {
                                style: style_pat!(next_c)
                            }),
                        }
                    },
                    _ => {
                        last_val.push(c);
                    },
                }
            }
            if last_val.len() != 0 {
                args.push(FmtType::Const(last_val));
            }
            Ok(args.into())
        }
        pub fn fmt_str<S: Display + Debug>(&self, strs: &[S]) -> String {
            let mut res = String::new();
            let mut idx = 0;
            for i in &self.args {
                res.push_str(&i.fmt_str(&mut idx, strs));
            }
            res
        }
    }
}
pub mod traits {
    pub trait FilterOutLongEmpty {
        type Output;
        fn filter_out_long_empty(&self, count: usize) -> Self::Output;
    }
    impl FilterOutLongEmpty for &str {
        type Output = String;
        /// 过滤多余的空白符
        /// # Examples
        /// ```
        /// use baidu_fanyi::traits::FilterOutLongEmpty;
        /// assert_eq!(&"a   b".filter_out_long_empty(0), "ab");
        /// assert_eq!(&"a   b".filter_out_long_empty(1), "a b");
        /// assert_eq!(&"a   b".filter_out_long_empty(2), "a  b");
        /// assert_eq!(&"a   b".filter_out_long_empty(3), "a   b");
        /// assert_eq!(&"a   b".filter_out_long_empty(4), "a   b");
        fn filter_out_long_empty(&self, count: usize) -> Self::Output {
            let mut res = String::with_capacity(self.len());
            let mut continue_count: usize = 0;
            if count == 0 {
                #[inline]
                fn filter(char: &char) -> bool {
                    ! char.is_whitespace()
                }
                res.extend(self.chars().filter(filter))
            } else {
                for char in self.chars() {
                    if char.is_whitespace() {
                        continue_count += 1
                    } else {
                        continue_count = 0
                    }
                    if continue_count <= count {
                        res.push(char)
                    }
                }
            }
            res
        }
    }
}
