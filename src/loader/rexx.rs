// SPDX-License-Identifier: GPL-3.0-or-later
//
// REXX, the built-in CMD.EXE's scripting language (ADR-0081, ADR-0083,
// ADR-0084):
// our own interpreter, written from IBM's public REXX documentation,
// Cowlishaw's The REXX Language and ANSI X3.274-1996. A `.CMD` file whose
// first line starts with `/*` runs here; its commands go to the shell.

pub mod builtins;
pub mod condition;
pub mod embed;
pub mod expr;
pub mod interp;
pub mod lexer;
pub mod number;
pub mod program;
pub mod rexxutil;
pub mod stream;
pub mod template;
pub mod trace;

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::{HashMap, VecDeque};

    use super::embed::*;
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
        /// HALT: 0 never; n: the program is asked to stop from the n-th
        /// look on.
        halt: Cell<u32>,
        /// Handlers it has: `SUBCOM:env`, `FUNC:name`, `EXIT:SAY`.
        accepts: Vec<&'static str>,
        /// What its handlers do, in turn; the calls made; the pool's answers.
        script: VecDeque<CallbackEvent>,
        calls: Vec<Callback>,
        answers: Vec<Vec<PoolAnswer>>,
        /// The session queue.
        session_queue: VecDeque<Vec<u8>>,
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
        fn interrupted(&self) -> bool {
            match self.halt.get() {
                0 => false,
                1 => true,
                n => { self.halt.set(n - 1); false }
            }
        }
        fn now(&self) -> Option<(LocalTime, u8)> {
            let mut t = LocalTime::new(2026, 11, 20, 13, 5, 9);
            t.weekday = 5;
            Some((t, 42))
        }
        fn external(&mut self, name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
            self.externals.get(name).map(|s| (s.clone(), [b"C:\\", name, b".CMD"].concat()))
        }
        fn get_env(&self, name: &[u8]) -> Option<Vec<u8>> { self.env.get(&name.to_ascii_uppercase()).cloned() }
        fn has_handler(&mut self, cb: &Callback) -> bool {
            let key = match cb {
                Callback::Subcom { env, .. } => format!("SUBCOM:{}", String::from_utf8_lossy(env)),
                Callback::Function { name, .. } => format!("FUNC:{}", String::from_utf8_lossy(name)),
                Callback::Exit(ExitCall::Say(_)) => "EXIT:SAY".to_string(),
                Callback::Exit(_) => "EXIT:OTHER".to_string(),
            };
            self.accepts.contains(&key.as_str())
        }
        fn callback_begin(&mut self, cb: Callback) { self.calls.push(cb); }
        fn callback_next(&mut self) -> CallbackEvent {
            self.script.pop_front().unwrap_or(CallbackEvent::Done(CallbackResult::Exit { handled: false, raise_error: false, value: None }))
        }
        fn pool_answer(&mut self, answers: Vec<PoolAnswer>) { self.answers.push(answers); }
        fn queue_to_host(&mut self, queue: VecDeque<Vec<u8>>) { self.session_queue = queue; }
        fn queue_from_host(&mut self) -> VecDeque<Vec<u8>> { std::mem::take(&mut self.session_queue) }
        fn set_env(&mut self, name: &[u8], value: Option<&[u8]>) {
            match value {
                Some(v) => { self.env.insert(name.to_ascii_uppercase(), v.to_vec()); }
                None => { self.env.remove(&name.to_ascii_uppercase()); }
            }
        }
    }

    /// Run `src` as the shell does, on a thread with the interpreter's
    /// stack.
    fn run_with(host: &mut TestHost, src: &str, arg: &str) -> Result<Option<String>, RexxError> {
        std::thread::scope(|scope| {
            std::thread::Builder::new().stack_size(64 * 1024 * 1024).spawn_scoped(scope, || {
                run_program(host, src.as_bytes(), b"C:\\TEST.CMD", vec![Some(arg.as_bytes().to_vec())], CallType::Command)
                    .map(|v| v.map(|v| String::from_utf8_lossy(&v).into_owned()))
            }).unwrap().join().unwrap()
        })
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
        // TRACE N (the default) shows the failure: an unknown environment.
        assert_eq!(h.out, ["0", "3", "     1 *-* address other 'x'", "       +++ RC(-3) +++", "-3", "CMD"]);
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
    fn conditions_are_trapped() {
        assert_eq!(say("signal on syntax\nsay 1 + 'a'\nexit\nsyntax: say 'trapped' rc sigl condition('C') condition('S')"),
                   ["trapped 41 2 SYNTAX OFF"]);
        assert_eq!(say("signal on novalue; say x; exit\nnovalue: say condition('D') condition('I')"), ["X SIGNAL"]);
        assert_eq!(say("signal on novalue; s.1 = 1; say s.1; say s.2; exit\nnovalue: say 'nv' condition('D')"), ["1", "nv S.2"]);
        assert_eq!(say("signal on novalue; signal off novalue; say y"), ["Y"]);
        assert_eq!(say("call on error\n'exit 3'\nsay 'after' rc\nexit\nerror: say 'error' condition('D') condition('S') sigl; return"),
                   ["error exit 3 DELAY 2", "after 3"]);
        assert_eq!(say("trace o; signal on failure; address other 'x'; exit\nfailure: say 'failed' rc"), ["failed -3"]);
        assert_eq!(say("trace o; signal on error; address other 'x'; exit\nerror: say condition('C')"), ["ERROR"], "an untrapped FAILURE raises ERROR");
        assert_eq!(say("'exit 1'; say 'ignored'"), ["ignored"], "no trap: nothing happens");
        assert_eq!(say("call r; say z; exit\nr: signal on novalue; return"), ["Z"], "traps come back after a routine");
        assert_eq!(say("signal on syntax name oops; x = 1/0; exit\noops: say 'oops'"), ["oops"]);
        assert_eq!(say("call on error name e1; 'exit 2'; 'exit 4'; exit\ne1: say rc; return"), ["2", "4"]);
        let src = "signal on syntax\ncall r\nexit\nr: signal off syntax; say 1 + 'a'; return\nsyntax: say 'caller'";
        assert_eq!(error(src), (41, 4), "a routine's untrapped error is not its caller's");
        assert_eq!(error("signal on syntax name nowhere; say 1 + 'a'"), (16, 1));
    }

    #[test]
    fn halt_is_raised_once() {
        let run = |src: &str, halt_at: u32| {
            let mut h = TestHost::default();
            h.halt.set(halt_at);
            let r = run_with(&mut h, src, "");
            (h.out, r.map_err(|e| e.code))
        };
        assert_eq!(run("signal on halt; do forever; nop; end\nhalt: say 'halted' condition('C')", 2), (vec!["halted HALT".to_string()], Ok(None)));
        assert_eq!(run("call on halt; do i = 1 to 3; nop; end; say 'done'; exit\nhalt: say 'h'; return", 2),
                   (vec!["h".to_string(), "done".to_string()], Ok(None)));
        assert_eq!(run("say 'never'", 1), (vec![], Err(4)), "an untrapped HALT ends the program");
        assert_eq!(run("signal on halt; nop\nhalt: do forever; end", 2).1, Err(4), "a program that goes on is ended");
    }

    #[test]
    fn trace_writes_clauses_and_values() {
        assert_eq!(say("trace r; x = 1 + 2"), ["     1 *-* x = 1 + 2", "       >>>   \"3\""]);
        assert_eq!(say("a = 2; trace i; say a + 1"),
                   ["     1 *-* say a + 1", "       >V>   \"2\"", "       >L>   \"1\"", "       >O>   \"3\"", "       >>>   \"3\"", "3"]);
        assert_eq!(say("trace e; 'exit 3'; 'exit 0'"), ["     1 *-* 'exit 3'", "       +++ RC(3) +++"]);
        assert_eq!(say("trace l; call f; exit\nf: return"), ["     2 *-* f:"]);
        assert_eq!(say("trace a; x = 1,\n  + 1"), ["     1 *-* x = 1,", "       *,* + 1"]);
        assert_eq!(say("trace r; parse value 'a b' with x ."), ["     1 *-* parse value 'a b' with x .", "       >>>   \"a b\"",
                       "       >>>   \"a\"", "       >.>   \"b\""]);
        assert_eq!(say("say trace('O') trace(); trace ?r; say trace(); trace off; say trace()"), ["N O", "     1 *-* say trace()", "       >>>   \"?R\"", "?R",
                   "     1 *-* trace off", "       >>>   \"off\"", "O"]);
        assert_eq!(say("trace 5; trace -1; say 'numbers are ignored'"), ["numbers are ignored"]);
        assert_eq!(error("trace x"), (24, 1));
    }

    #[test]
    fn engineering_notation() {
        assert_eq!(say("numeric form engineering; say 1.23E+10 * 1 form(); numeric form; say 1.23E+10 * 1"), ["12.3E+9 ENGINEERING", "1.23E+10"]);
    }

    #[test]
    fn rexxutil_functions_are_registered() {
        assert_eq!(say("say rxfuncquery('SysCls') rxfuncadd('MyVer', 'RexxUtil', 'SysOS2Ver') myver() rxfuncdrop('MyVer') rxfuncquery('MyVer') rxfuncadd('X', 'RXSOCK', 'X')"),
                   ["0 0 2.45 0 1 1"]);
        assert_eq!(say("call RxFuncAdd 'SysLoadFuncs', 'RexxUtil', 'SysLoadFuncs'; call SysLoadFuncs; say SysOS2Ver() SysBootDrive()"), ["2.45 C:"]);
        assert_eq!(error("call SysDropFuncs; call SysCls"), (43, 1));
        assert_eq!(say("call SysSleep 0; say 'slept'"), ["slept"]);
    }

    #[test]
    fn default_streams() {
        let mut h = TestHost { input: VecDeque::from([b"ab".to_vec(), b"cdef".to_vec()]), ..Default::default() };
        run_with(&mut h, "say lines() linein() chars(); say charin(,,3); pull rest; say rest lines(); call lineout , 'out'", "").unwrap();
        assert_eq!(h.out, ["1 ab 6", "cde", "F 0", "out"]);
        let mut h = TestHost::default();
        run_with(&mut h, "signal on notready; x = linein(); exit\nnotready: say 'nr' condition('D')", "").unwrap();
        assert_eq!(h.out, ["nr STDIN"]);
        assert_eq!(say("say stream('STDOUT') stream('nofile', 'D') lineout('nofile', 'x') linein('nofile')'|'"), ["READY UNKNOWN: 1 |"],
                   "a host without files: no stream opens");
    }

    fn req(code: u8, name: &str, value: &str) -> PoolRequest {
        PoolRequest { code, name: name.as_bytes().to_vec(), value: value.as_bytes().to_vec() }
    }

    fn answer(ret: u8, value: Option<&str>) -> PoolAnswer {
        PoolAnswer { ret, name: None, value: value.map(|v| v.as_bytes().to_vec()) }
    }

    #[test]
    fn handlers_are_called_and_use_the_variable_pool() {
        let mut h = TestHost { accepts: vec!["SUBCOM:TEST", "FUNC:MYFUNC", "EXIT:SAY"], ..Default::default() };
        h.script = VecDeque::from([
            CallbackEvent::Pool(vec![
                req(RXSHV_SET, "FROMHANDLER", "set"), req(RXSHV_FETCH, "A", ""), req(RXSHV_SYFET, "s.i", ""),
                req(RXSHV_FETCH, "S.2", ""), req(RXSHV_PRIV, "PARM", ""), req(RXSHV_DROPV, "B", ""),
                req(RXSHV_SET, "bad name", "x"), req(RXSHV_FETCH, "NEVER", ""), req(99, "A", ""),
            ]),
            CallbackEvent::Done(CallbackResult::Command { rc: b"7".to_vec(), error: true, failure: false }),
            CallbackEvent::Done(CallbackResult::Function(Ok(Some(b"42".to_vec())))),
            CallbackEvent::Done(CallbackResult::Exit { handled: true, raise_error: false, value: None }),
        ]);
        run_with(&mut h, "a = 5; b = 1; s.2 = 'two'; i = 2\naddress test 'hello'\nx = myfunc(1)\nsay rc fromhandler x symbol('b')", "").unwrap();
        assert_eq!(h.calls[0], Callback::Subcom { env: b"TEST".to_vec(), command: b"hello".to_vec() });
        assert_eq!(h.calls[1], Callback::Function { name: b"MYFUNC".to_vec(), args: vec![Some(b"1".to_vec())], subroutine: false });
        assert_eq!(h.calls[2], Callback::Exit(ExitCall::Say(b"7 set 42 LIT".to_vec())), "the exit took SAY's line");
        assert!(h.out.is_empty(), "a handled SAY is not written");
        assert_eq!(h.answers[0], [answer(RXSHV_NEWV, None), answer(RXSHV_OK, Some("5")), answer(RXSHV_OK, Some("two")),
                                  answer(RXSHV_OK, Some("two")), answer(RXSHV_OK, Some("1")), answer(RXSHV_OK, None),
                                  answer(RXSHV_BADN, None), answer(RXSHV_NEWV, Some("NEVER")), answer(RXSHV_BADF, None)]);
    }

    #[test]
    fn the_pool_walks_the_variables_and_handlers_can_fail() {
        let mut h = TestHost { accepts: vec!["SUBCOM:TEST"], ..Default::default() };
        h.script = VecDeque::from([
            CallbackEvent::Pool(vec![req(RXSHV_NEXTV, "", ""), req(RXSHV_NEXTV, "", ""), req(RXSHV_NEXTV, "", "")]),
            CallbackEvent::Done(CallbackResult::Command { rc: b"-1".to_vec(), error: false, failure: true }),
        ]);
        run_with(&mut h, "a = 1; s.x = 2\ntrace o; signal on failure; address test 'x'; exit\nfailure: say 'failed' rc", "").unwrap();
        let mut seen: Vec<(Vec<u8>, Vec<u8>)> = h.answers[0][..2].iter().map(|a| (a.name.clone().unwrap(), a.value.clone().unwrap())).collect();
        seen.sort();
        assert_eq!(seen, [(b"A".to_vec(), b"1".to_vec()), (b"S.X".to_vec(), b"2".to_vec())]);
        assert_eq!(h.answers[0][2].ret, RXSHV_LVAR, "the walk ends");
        assert_eq!(h.out, ["failed -1"], "a handler's failure raises FAILURE");
        let mut h = TestHost { accepts: vec!["FUNC:BAD"], ..Default::default() };
        h.script = VecDeque::from([CallbackEvent::Done(CallbackResult::Function(Err(())))]);
        assert_eq!(run_with(&mut h, "x = bad()", "").unwrap_err().code, 40);
    }

    #[test]
    fn the_session_queue_is_the_hosts() {
        let mut h = TestHost { session_queue: VecDeque::from([b"from host".to_vec()]), ..Default::default() };
        run_with(&mut h, "pull x; say x; queue 'back'", "").unwrap();
        assert_eq!(h.out, ["FROM HOST"]);
        assert_eq!(h.session_queue, [b"back".to_vec()], "the queue goes back at the end");
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
