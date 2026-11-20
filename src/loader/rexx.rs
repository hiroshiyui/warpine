// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX, the built-in CMD.EXE's scripting language (ADR-0081, ADR-0083):
// our own interpreter, written from IBM's public REXX documentation,
// Cowlishaw's The REXX Language and ANSI X3.274-1996. A `.CMD` file whose
// first line starts with `/*` runs here; its commands go to the shell.

pub mod builtins;
pub mod expr;
pub mod interp;
pub mod lexer;
pub mod number;
pub mod program;
pub mod template;

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};

    use super::interp::{run_program, CallType, Host, RexxError};
    use crate::loader::clock::LocalTime;

    /// A host that records SAY's lines and the commands, gives input
    /// lines, a fixed clock (Friday 20 November 2026, 13:05:09.42), an
    /// environment and external routines.
    #[derive(Default)]
    struct TestHost {
        out: Vec<String>,
        commands: Vec<(String, String)>,
        input: VecDeque<Vec<u8>>,
        env: HashMap<Vec<u8>, Vec<u8>>,
        externals: HashMap<Vec<u8>, Vec<u8>>,
    }

    impl Host for TestHost {
        fn say(&mut self, line: &[u8]) { self.out.push(String::from_utf8_lossy(line).into_owned()); }
        fn command(&mut self, env: &[u8], command: &[u8]) -> Option<i64> {
            if env != b"CMD" { return None; }
            let c = String::from_utf8_lossy(command).into_owned();
            self.commands.push((String::from_utf8_lossy(env).into_owned(), c.clone()));
            // `exit N` returns N.
            Some(c.strip_prefix("exit ").and_then(|n| n.parse().ok()).unwrap_or(0))
        }
        fn read_line(&mut self) -> Option<Vec<u8>> { self.input.pop_front() }
        fn interrupted(&self) -> bool { false }
        fn now(&self) -> Option<(LocalTime, u8)> {
            let mut t = LocalTime::new(2026, 11, 20, 13, 5, 9);
            t.weekday = 5;
            Some((t, 42))
        }
        fn external(&mut self, name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
            self.externals.get(name).map(|s| (s.clone(), [b"C:\\", name, b".CMD"].concat()))
        }
        fn get_env(&self, name: &[u8]) -> Option<Vec<u8>> { self.env.get(&name.to_ascii_uppercase()).cloned() }
        fn set_env(&mut self, name: &[u8], value: Option<&[u8]>) {
            match value {
                Some(v) => { self.env.insert(name.to_ascii_uppercase(), v.to_vec()); }
                None => { self.env.remove(&name.to_ascii_uppercase()); }
            }
        }
    }

    fn run_with(host: &mut TestHost, src: &str, arg: &str) -> Result<Option<String>, RexxError> {
        run_program(host, src.as_bytes(), b"C:\\TEST.CMD", vec![Some(arg.as_bytes().to_vec())], CallType::Command)
            .map(|v| v.map(|v| String::from_utf8_lossy(&v).into_owned()))
    }

    /// Run `src`; returns SAY's lines.
    fn say(src: &str) -> Vec<String> {
        let mut h = TestHost::default();
        if let Err(e) = run_with(&mut h, src, "") { panic!("{e:?} in\n{src}"); }
        h.out
    }

    fn error(src: &str) -> (u32, usize) {
        let mut h = TestHost::default();
        let e = run_with(&mut h, src, "").expect_err("an error");
        (e.code, e.line)
    }

    #[test]
    fn expressions_and_say() {
        assert_eq!(say("say 1 + 2 * 3; say (1+2)*3; say 7 / 2; say 7 % 2; say 7 // 2; say 2**10"), ["7", "9", "3.5", "3", "1", "1024"]);
        assert_eq!(say("x = 'abc'; say x'def' x 'g'||'h'; say y"), ["abcdef abc gh", "Y"]);
        assert_eq!(say("say 1 = 1.0; say 'a' = ' a '; say 'a' == ' a '; say 10 > 9; say '10' >> '9'"), ["1", "1", "0", "1", "0"]);
        assert_eq!(say("say \\0; say 1 & 0 | 1; say 1 && 1"), ["1", "1", "0"]);
        assert_eq!(say("say -2**2; say 2**3**2; say 1.50 + 1; say 1.5 * 2"), ["4", "64", "2.50", "3.0"]);
        assert_eq!(say("numeric digits 20; say 1/3; numeric digits; say 1/3"), ["0.33333333333333333333", "0.333333333"]);
        assert_eq!(say("say 999999999 + 1"), ["1.00000000E+9"]);
    }

    #[test]
    fn control_structures() {
        assert_eq!(say("do i = 1 to 3; say i; end"), ["1", "2", "3"]);
        assert_eq!(say("do i = 10 to 1 by -4; say i; end i"), ["10", "6", "2"]);
        assert_eq!(say("do 2; say 'x'; end; do i = 1 for 2; say i; end"), ["x", "x", "1", "2"]);
        assert_eq!(say("i = 0; do while i < 2; i = i + 1; say i; end"), ["1", "2"]);
        assert_eq!(say("i = 5; do until i > 5; say i; i = i + 1; end"), ["5"]);
        assert_eq!(say("do i = 1 to 5; if i = 2 then iterate; if i = 4 then leave; say i; end; say 'i='i"), ["1", "3", "i=4"]);
        assert_eq!(say("do i = 1 to 2; do j = 1 to 3; if j = 2 then iterate i; say i j; end; end"), ["1 1", "2 1"]);
        assert_eq!(say("n = 0; do forever; n = n + 1; if n = 3 then leave; end; say n"), ["3"]);
        assert_eq!(say("x = 2\nselect\n when x = 1 then say 'one'\n when x = 2 then do; say 'two'; say 'too'; end\n otherwise say 'many'\nend"), ["two", "too"]);
        assert_eq!(say("select; when 0 then nop; otherwise say 'other'; end"), ["other"]);
        assert_eq!(say("if 1 then if 0 then say 'a'; else say 'b'; else say 'c'"), ["b"]);
        assert_eq!(say("if 0 then say 'a'\nelse do\n say 'b'\nend\nsay 'c'"), ["b", "c"]);
    }

    #[test]
    fn routines_and_variables() {
        let src = "say fact(5); call twice 21; say result; exit\n\
                   fact: procedure; arg n; if n <= 1 then return 1; return n * fact(n - 1)\n\
                   twice: return arg(1) * 2";
        assert_eq!(say(src), ["120", "42"]);
        let src = "a = 1; b = 2; call p; say a b c\nexit\np: procedure expose a; a = 10; b = 20; c = 30; return";
        assert_eq!(say(src), ["10 2 C"]);
        let src = "s.1 = 'x'; s.2 = 'y'; i = 2; say s.i s.3; s. = 'd'; say s.1 s.9; drop s.9; say s.9";
        assert_eq!(say(src), ["y S.3", "d d", "S.9"]);
        let src = "list = 'a b'; a = 1; call q; say a b\nexit\nq: procedure expose (list); a = 5; b = 6; return";
        assert_eq!(say(src), ["5 6"]);
        assert_eq!(say("say f(1,,3)\nexit\nf: return arg() arg(2,'o') arg(3)"), ["3 1 3"]);
        assert_eq!(say("x = 1; drop x; say x; say symbol('x') symbol('y.1') symbol('1x') symbol('a b')"), ["X", "LIT LIT LIT BAD"]);
        assert_eq!(say("signal there; say 'skipped'\nthere: say 'here'"), ["here"]);
        assert_eq!(say("x = 'say 1+1; y = 3'; interpret x; say y"), ["2", "3"]);
        assert_eq!(say("call e; say 'not reached'\ne: exit 7").len(), 0);
    }

    #[test]
    fn parse_templates() {
        assert_eq!(say("parse value 'a b c d' with x y z; say x'|'y'|'z"), ["a|b|c d"]);
        assert_eq!(say("parse value '  Hello   world  ' with a b; say '['a'|'||b']'"), ["[Hello|  world  ]"]);
        assert_eq!(say("parse value 'k=v;rest' with key '=' val ';' tail; say key val tail"), ["k v rest"]);
        assert_eq!(say("parse value 'abcdefgh' with 3 x 5 y +2 z; say x y z"), ["cd ef gh"]);
        assert_eq!(say("parse value 'abc' with 1 x 1 y; say x y"), ["abc abc"]);
        assert_eq!(say("d = ','; parse value 'a,b' with p (d) q; say p q"), ["a b"]);
        assert_eq!(say("parse upper value 'Mixed Case' with w .; say w"), ["MIXED"]);
        let mut h = TestHost::default();
        run_with(&mut h, "parse arg first rest; say first; say rest; arg up; say up", "one two  three").unwrap();
        assert_eq!(h.out, ["one", "two  three", "ONE TWO  THREE"]);
        let mut h = TestHost { input: VecDeque::from([b"typed in".to_vec()]), ..Default::default() };
        run_with(&mut h, "push 'pushed'; queue 'queued'; say queued(); pull a; pull b; parse pull c; say a; say b; say c", "").unwrap();
        assert_eq!(h.out, ["2", "PUSHED", "QUEUED", "typed in"]);
        assert_eq!(say("parse source s; say s; parse version v; say word(v, 2)"), ["OS/2 COMMAND C:\\TEST.CMD", "4.00"]);
    }

    #[test]
    fn commands_go_to_the_environment() {
        let mut h = TestHost::default();
        run_with(&mut h, "'dir /w'; say rc; 'exit 3'; say rc; address cmd 'ver'; address other 'x'; say rc; address; say address()", "").unwrap();
        assert_eq!(h.commands.iter().map(|(_, c)| c.as_str()).collect::<Vec<_>>(), ["dir /w", "exit 3", "ver"]);
        assert_eq!(h.out, ["0", "3", "-3", "CMD"]);
        let mut h = TestHost::default();
        h.env.insert(b"PATH".to_vec(), b"C:\\".to_vec());
        run_with(&mut h, "say value('path',,'OS2ENVIRONMENT'); call value 'NEW', 'set', 'OS2ENVIRONMENT'", "").unwrap();
        assert_eq!(h.out, ["C:\\"]);
        assert_eq!(h.env.get(b"NEW".as_slice()).map(Vec::as_slice), Some(b"set".as_slice()));
        let mut h = TestHost::default();
        h.externals.insert(b"HELPER".to_vec(), b"/* */ parse arg x; return x * 3".to_vec());
        run_with(&mut h, "say helper(14)", "").unwrap();
        assert_eq!(h.out, ["42"]);
        let mut h = TestHost::default();
        assert_eq!(run_with(&mut h, "exit 'done'", "").unwrap().as_deref(), Some("done"));
    }

    #[test]
    fn builtin_functions() {
        assert_eq!(say("say substr('hello', 2, 3) left('ab', 4, '.') right('ab', 4, '.') center('x', 5, '*')"), ["ell ab.. ..ab **x**"]);
        assert_eq!(say("say length('abc') reverse('abc') copies('ab', 3) pos('l', 'hello') lastpos('l', 'hello')"), ["3 cba ababab 3 4"]);
        assert_eq!(say("say word('a b c', 2) words('a  b c') subword('a b c d', 2, 2) wordpos('c d', 'a b c d') delword('a b c', 2, 1)"), ["b 3 b c 3 a c"]);
        assert_eq!(say("say strip('  x  ') strip('xxyxx', , 'x') space(' a   b ') translate('abc') translate('abc', 'xy', 'ab')"), ["x y a b ABC xyc"]);
        assert_eq!(say("say c2x('AB') x2c('4142') d2x(255) x2d('FF') c2d('A') d2c(65) b2x('0100 0001') x2b('41')"), ["4142 AB FF 255 65 A 41 01000001"]);
        assert_eq!(say("say x2d('FF', 2) d2x(-1, 4) c2d('FF'x, 1)"), ["-1 FFFF -1"]);
        assert_eq!(say("say abs(-3) sign(-2) max(1, 7, 3) min(4, 2) trunc(3.789, 2) format(3.14159, 3, 2) format(2.5, , 0)"), ["3 -1 7 2 3.78   3.14 3"]);
        assert_eq!(say("say datatype(12) datatype('a1') datatype('12', 'W') datatype('ff', 'X') datatype('Ab', 'M') verify('123', '0123456789')"), ["NUM CHAR 1 1 1 0"]);
        assert_eq!(say("say compare('abc', 'abd') abbrev('PRINT', 'PRI') insert('X', 'abc', 1) overlay('XY', 'abcd', 2) delstr('abcd', 2, 2)"), ["3 1 aXbc aXYd ad"]);
        assert_eq!(say("say date() date('S') date('W') date('B') time() time('M')"), ["20 Nov 2026 20261120 Friday 739939 13:05:09 785"]);
        assert_eq!(say("say digits() fuzz() form() errortext(16) random(5, 5) length(xrange('a', 'e'))"), ["9 0 SCIENTIFIC Label not found 5 5"]);
        assert_eq!(say("say (bitand('73'x, '27'x) == '23'x) (bitor('15'x, '24'x) == '35'x)"), ["1 1"]);
    }

    #[test]
    fn errors_have_codes_and_lines() {
        assert_eq!(error("say 1\nsay 1 + 'a'"), (41, 2));
        assert_eq!(error("signal nowhere"), (16, 1));
        assert_eq!(error("x = f()"), (43, 1));
        assert_eq!(error("if 2 then nop"), (34, 1));
        assert_eq!(error("say 1/0"), (42, 1));
        assert_eq!(error("say substr('a', 0)"), (40, 1));
        assert_eq!(error("\n\ndo\nsay 1"), (14, 3));
        assert_eq!(error("procedure"), (17, 1));
        assert_eq!(error("leave"), (28, 1));
        assert_eq!(error("x = r()\nexit\nr: return"), (44, 1));
        assert_eq!(error("call r\nexit\nr: call r").0, 11, "recursion is bounded");
        assert_eq!(error("x = copies('x', 99999999)").0, 5, "strings are bounded");
    }
}
