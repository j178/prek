// Copyright (c) 2017 Chris Kuehl, Anthony Sottile
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use std::io::{BufRead, Read};
use std::iter::FromIterator;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::Result;
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;

#[derive(Clone, Default)]
pub(crate) struct TagSet(SmallVec<[&'static str; 8]>);

impl TagSet {
    fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, tag: &'static str) -> bool {
        if self.0.contains(&tag) {
            false
        } else {
            self.0.push(tag);
            true
        }
    }

    fn extend_from_iter<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = &'static str>,
    {
        for tag in iter {
            self.insert(tag);
        }
    }

    pub(crate) fn contains(&self, needle: &str) -> bool {
        self.0.contains(&needle)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.iter().copied()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn with_added(mut self, extra: &[&'static str]) -> Self {
        self.extend_from_iter(extra.iter().copied());
        self
    }
}

impl Extend<&'static str> for TagSet {
    fn extend<I: IntoIterator<Item = &'static str>>(&mut self, iter: I) {
        self.extend_from_iter(iter);
    }
}

impl FromIterator<&'static str> for TagSet {
    fn from_iter<I: IntoIterator<Item = &'static str>>(iter: I) -> Self {
        let mut set = TagSet::new();
        set.extend(iter);
        set
    }
}

impl<const N: usize> From<[&'static str; N]> for TagSet {
    fn from(tags: [&'static str; N]) -> Self {
        tags.into_iter().collect()
    }
}

#[derive(Default)]
struct TagMap(FxHashMap<&'static str, TagSet>);

impl TagMap {
    fn get(&self, key: &str) -> Option<&TagSet> {
        self.0.get(key)
    }

    fn insert(&mut self, key: &'static str, value: TagSet) {
        self.0.insert(key, value);
    }

    fn clone_key(&self, key: &str) -> TagSet {
        self.0
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("TagMap missing key: {key}"))
    }

    fn values(&self) -> impl Iterator<Item = &TagSet> {
        self.0.values()
    }
}

mod tags {
    pub const DIRECTORY: &str = "directory";
    pub const SYMLINK: &str = "symlink";
    pub const SOCKET: &str = "socket";
    pub const FIFO: &str = "fifo";
    pub const BLOCK_DEVICE: &str = "block-device";
    pub const CHARACTER_DEVICE: &str = "character-device";
    pub const FILE: &str = "file";
    pub const EXECUTABLE: &str = "executable";
    pub const NON_EXECUTABLE: &str = "non-executable";
    pub const TEXT: &str = "text";
    pub const BINARY: &str = "binary";
}

fn by_extension() -> &'static TagMap {
    static EXTENSIONS: OnceLock<TagMap> = OnceLock::new();
    EXTENSIONS.get_or_init(|| {
        let mut map = TagMap::default();
        map.insert("adoc", TagSet::from([tags::TEXT, "asciidoc"]));
        map.insert("ai", TagSet::from([tags::BINARY, "adobe-illustrator"]));
        map.insert("aj", TagSet::from([tags::TEXT, "aspectj"]));
        map.insert("asciidoc", TagSet::from([tags::TEXT, "asciidoc"]));
        map.insert("apinotes", TagSet::from([tags::TEXT, "apinotes"]));
        map.insert("asar", TagSet::from([tags::BINARY, "asar"]));
        map.insert("astro", TagSet::from([tags::TEXT, "astro"]));
        map.insert("avif", TagSet::from([tags::BINARY, "image", "avif"]));
        map.insert("avsc", TagSet::from([tags::TEXT, "avro-schema"]));
        map.insert("bash", TagSet::from([tags::TEXT, "shell", "bash"]));
        map.insert("bat", TagSet::from([tags::TEXT, "batch"]));
        map.insert("bats", TagSet::from([tags::TEXT, "shell", "bash", "bats"]));
        map.insert("bazel", TagSet::from([tags::TEXT, "bazel"]));
        map.insert("beancount", TagSet::from([tags::TEXT, "beancount"]));
        map.insert("bib", TagSet::from([tags::TEXT, "bib"]));
        map.insert("bmp", TagSet::from([tags::BINARY, "image", "bitmap"]));
        map.insert("bz2", TagSet::from([tags::BINARY, "bzip2"]));
        map.insert("bzl", TagSet::from([tags::TEXT, "bazel"]));
        map.insert("c", TagSet::from([tags::TEXT, "c"]));
        map.insert("c++", TagSet::from([tags::TEXT, "c++"]));
        map.insert("c++m", TagSet::from([tags::TEXT, "c++"]));
        map.insert("cc", TagSet::from([tags::TEXT, "c++"]));
        map.insert("ccm", TagSet::from([tags::TEXT, "c++"]));
        map.insert("cfg", TagSet::from([tags::TEXT]));
        map.insert("chs", TagSet::from([tags::TEXT, "c2hs"]));
        map.insert("cjs", TagSet::from([tags::TEXT, "javascript"]));
        map.insert("clj", TagSet::from([tags::TEXT, "clojure"]));
        map.insert("cljc", TagSet::from([tags::TEXT, "clojure"]));
        map.insert(
            "cljs",
            TagSet::from([tags::TEXT, "clojure", "clojurescript"]),
        );
        map.insert("cmake", TagSet::from([tags::TEXT, "cmake"]));
        map.insert("cnf", TagSet::from([tags::TEXT]));
        map.insert("coffee", TagSet::from([tags::TEXT, "coffee"]));
        map.insert("conf", TagSet::from([tags::TEXT]));
        map.insert("cpp", TagSet::from([tags::TEXT, "c++"]));
        map.insert("cppm", TagSet::from([tags::TEXT, "c++"]));
        map.insert("cr", TagSet::from([tags::TEXT, "crystal"]));
        map.insert("crt", TagSet::from([tags::TEXT, "pem"]));
        map.insert("cs", TagSet::from([tags::TEXT, "c#"]));
        map.insert("csproj", TagSet::from([tags::TEXT, "xml", "csproj"]));
        map.insert("csh", TagSet::from([tags::TEXT, "shell", "csh"]));
        map.insert("cson", TagSet::from([tags::TEXT, "cson"]));
        map.insert("css", TagSet::from([tags::TEXT, "css"]));
        map.insert("csv", TagSet::from([tags::TEXT, "csv"]));
        map.insert("cu", TagSet::from([tags::TEXT, "cuda"]));
        map.insert("cue", TagSet::from([tags::TEXT, "cue"]));
        map.insert("cuh", TagSet::from([tags::TEXT, "cuda"]));
        map.insert("cxx", TagSet::from([tags::TEXT, "c++"]));
        map.insert("cxxm", TagSet::from([tags::TEXT, "c++"]));
        map.insert("cylc", TagSet::from([tags::TEXT, "cylc"]));
        map.insert("dart", TagSet::from([tags::TEXT, "dart"]));
        map.insert("dbc", TagSet::from([tags::TEXT, "dbc"]));
        map.insert("def", TagSet::from([tags::TEXT, "def"]));
        map.insert("dll", TagSet::from([tags::BINARY]));
        map.insert("dtd", TagSet::from([tags::TEXT, "dtd"]));
        map.insert("ear", TagSet::from([tags::BINARY, "zip", "jar"]));
        map.insert("edn", TagSet::from([tags::TEXT, "clojure", "edn"]));
        map.insert("ejs", TagSet::from([tags::TEXT, "ejs"]));
        map.insert("ejson", TagSet::from([tags::TEXT, "json", "ejson"]));
        map.insert("env", TagSet::from([tags::TEXT, "dotenv"]));
        map.insert("eot", TagSet::from([tags::BINARY, "eot"]));
        map.insert("eps", TagSet::from([tags::BINARY, "eps"]));
        map.insert("erb", TagSet::from([tags::TEXT, "erb"]));
        map.insert("erl", TagSet::from([tags::TEXT, "erlang"]));
        map.insert("ex", TagSet::from([tags::TEXT, "elixir"]));
        map.insert("exe", TagSet::from([tags::BINARY]));
        map.insert("exs", TagSet::from([tags::TEXT, "elixir"]));
        map.insert("eyaml", TagSet::from([tags::TEXT, "yaml"]));
        map.insert("f03", TagSet::from([tags::TEXT, "fortran"]));
        map.insert("f08", TagSet::from([tags::TEXT, "fortran"]));
        map.insert("f90", TagSet::from([tags::TEXT, "fortran"]));
        map.insert("f95", TagSet::from([tags::TEXT, "fortran"]));
        map.insert("feature", TagSet::from([tags::TEXT, "gherkin"]));
        map.insert("fish", TagSet::from([tags::TEXT, "fish"]));
        map.insert("fits", TagSet::from([tags::BINARY, "fits"]));
        map.insert("gd", TagSet::from([tags::TEXT, "gdscript"]));
        map.insert("gemspec", TagSet::from([tags::TEXT, "ruby"]));
        map.insert("geojson", TagSet::from([tags::TEXT, "geojson", "json"]));
        map.insert("ggb", TagSet::from([tags::BINARY, "zip", "ggb"]));
        map.insert("gif", TagSet::from([tags::BINARY, "image", "gif"]));
        map.insert("go", TagSet::from([tags::TEXT, "go"]));
        map.insert("gotmpl", TagSet::from([tags::TEXT, "gotmpl"]));
        map.insert("gpx", TagSet::from([tags::TEXT, "gpx", "xml"]));
        map.insert("graphql", TagSet::from([tags::TEXT, "graphql"]));
        map.insert("gradle", TagSet::from([tags::TEXT, "groovy"]));
        map.insert("groovy", TagSet::from([tags::TEXT, "groovy"]));
        map.insert("gyb", TagSet::from([tags::TEXT, "gyb"]));
        map.insert("gyp", TagSet::from([tags::TEXT, "gyp", "python"]));
        map.insert("gypi", TagSet::from([tags::TEXT, "gyp", "python"]));
        map.insert("gz", TagSet::from([tags::BINARY, "gzip"]));
        map.insert("h", TagSet::from([tags::TEXT, "header", "c", "c++"]));
        map.insert("hbs", TagSet::from([tags::TEXT, "handlebars"]));
        map.insert("hcl", TagSet::from([tags::TEXT, "hcl"]));
        map.insert("hh", TagSet::from([tags::TEXT, "header", "c++"]));
        map.insert("hpp", TagSet::from([tags::TEXT, "header", "c++"]));
        map.insert("hrl", TagSet::from([tags::TEXT, "erlang"]));
        map.insert("hs", TagSet::from([tags::TEXT, "haskell"]));
        map.insert("htm", TagSet::from([tags::TEXT, "html"]));
        map.insert("html", TagSet::from([tags::TEXT, "html"]));
        map.insert("hxx", TagSet::from([tags::TEXT, "header", "c++"]));
        map.insert("icns", TagSet::from([tags::BINARY, "icns"]));
        map.insert("ico", TagSet::from([tags::BINARY, "icon"]));
        map.insert("ics", TagSet::from([tags::TEXT, "icalendar"]));
        map.insert("idl", TagSet::from([tags::TEXT, "idl"]));
        map.insert("idr", TagSet::from([tags::TEXT, "idris"]));
        map.insert("inc", TagSet::from([tags::TEXT, "inc"]));
        map.insert("ini", TagSet::from([tags::TEXT, "ini"]));
        map.insert("inl", TagSet::from([tags::TEXT, "inl", "c++"]));
        map.insert("ino", TagSet::from([tags::TEXT, "ino", "c++"]));
        map.insert("inx", TagSet::from([tags::TEXT, "xml", "inx"]));
        map.insert("ipynb", TagSet::from([tags::TEXT, "jupyter", "json"]));
        map.insert("ixx", TagSet::from([tags::TEXT, "c++"]));
        map.insert("j2", TagSet::from([tags::TEXT, "jinja"]));
        map.insert("jade", TagSet::from([tags::TEXT, "jade"]));
        map.insert("jar", TagSet::from([tags::BINARY, "zip", "jar"]));
        map.insert("java", TagSet::from([tags::TEXT, "java"]));
        map.insert("jenkins", TagSet::from([tags::TEXT, "groovy", "jenkins"]));
        map.insert(
            "jenkinsfile",
            TagSet::from([tags::TEXT, "groovy", "jenkins"]),
        );
        map.insert("jinja", TagSet::from([tags::TEXT, "jinja"]));
        map.insert("jinja2", TagSet::from([tags::TEXT, "jinja"]));
        map.insert("jl", TagSet::from([tags::TEXT, "julia"]));
        map.insert("jpeg", TagSet::from([tags::BINARY, "image", "jpeg"]));
        map.insert("jpg", TagSet::from([tags::BINARY, "image", "jpeg"]));
        map.insert("js", TagSet::from([tags::TEXT, "javascript"]));
        map.insert("json", TagSet::from([tags::TEXT, "json"]));
        map.insert("json5", TagSet::from([tags::TEXT, "json5"]));
        map.insert("jsonld", TagSet::from([tags::TEXT, "json", "jsonld"]));
        map.insert("jsonnet", TagSet::from([tags::TEXT, "jsonnet"]));
        map.insert("jsx", TagSet::from([tags::TEXT, "jsx"]));
        map.insert("key", TagSet::from([tags::TEXT, "pem"]));
        map.insert("kml", TagSet::from([tags::TEXT, "kml", "xml"]));
        map.insert("kt", TagSet::from([tags::TEXT, "kotlin"]));
        map.insert("kts", TagSet::from([tags::TEXT, "kotlin"]));
        map.insert("lean", TagSet::from([tags::TEXT, "lean"]));
        map.insert(
            "lektorproject",
            TagSet::from([tags::TEXT, "ini", "lektorproject"]),
        );
        map.insert("less", TagSet::from([tags::TEXT, "less"]));
        map.insert("lfm", TagSet::from([tags::TEXT, "lazarus", "lazarus-form"]));
        map.insert("lhs", TagSet::from([tags::TEXT, "literate-haskell"]));
        map.insert("libsonnet", TagSet::from([tags::TEXT, "jsonnet"]));
        map.insert("lidr", TagSet::from([tags::TEXT, "idris"]));
        map.insert("liquid", TagSet::from([tags::TEXT, "liquid"]));
        map.insert("lpi", TagSet::from([tags::TEXT, "lazarus", "xml"]));
        map.insert("lpr", TagSet::from([tags::TEXT, "lazarus", "pascal"]));
        map.insert("lr", TagSet::from([tags::TEXT, "lektor"]));
        map.insert("lua", TagSet::from([tags::TEXT, "lua"]));
        map.insert("m", TagSet::from([tags::TEXT, "objective-c"]));
        map.insert("m4", TagSet::from([tags::TEXT, "m4"]));
        map.insert("make", TagSet::from([tags::TEXT, "makefile"]));
        map.insert("manifest", TagSet::from([tags::TEXT, "manifest"]));
        map.insert("map", TagSet::from([tags::TEXT, "map"]));
        map.insert("markdown", TagSet::from([tags::TEXT, "markdown"]));
        map.insert("md", TagSet::from([tags::TEXT, "markdown"]));
        map.insert("mdx", TagSet::from([tags::TEXT, "mdx"]));
        map.insert("meson", TagSet::from([tags::TEXT, "meson"]));
        map.insert("metal", TagSet::from([tags::TEXT, "metal"]));
        map.insert("mib", TagSet::from([tags::TEXT, "mib"]));
        map.insert("mjs", TagSet::from([tags::TEXT, "javascript"]));
        map.insert("mk", TagSet::from([tags::TEXT, "makefile"]));
        map.insert("ml", TagSet::from([tags::TEXT, "ocaml"]));
        map.insert("mli", TagSet::from([tags::TEXT, "ocaml"]));
        map.insert("mm", TagSet::from([tags::TEXT, "c++", "objective-c++"]));
        map.insert("modulemap", TagSet::from([tags::TEXT, "modulemap"]));
        map.insert("mscx", TagSet::from([tags::TEXT, "xml", "musescore"]));
        map.insert("mscz", TagSet::from([tags::BINARY, "zip", "musescore"]));
        map.insert("mustache", TagSet::from([tags::TEXT, "mustache"]));
        map.insert("myst", TagSet::from([tags::TEXT, "myst"]));
        map.insert("ngdoc", TagSet::from([tags::TEXT, "ngdoc"]));
        map.insert("nim", TagSet::from([tags::TEXT, "nim"]));
        map.insert("nimble", TagSet::from([tags::TEXT, "nimble"]));
        map.insert("nims", TagSet::from([tags::TEXT, "nim"]));
        map.insert("nix", TagSet::from([tags::TEXT, "nix"]));
        map.insert("njk", TagSet::from([tags::TEXT, "nunjucks"]));
        map.insert("otf", TagSet::from([tags::BINARY, "otf"]));
        map.insert("p12", TagSet::from([tags::BINARY, "p12"]));
        map.insert("pas", TagSet::from([tags::TEXT, "pascal"]));
        map.insert("patch", TagSet::from([tags::TEXT, "diff"]));
        map.insert("pdf", TagSet::from([tags::BINARY, "pdf"]));
        map.insert("pem", TagSet::from([tags::TEXT, "pem"]));
        map.insert("php", TagSet::from([tags::TEXT, "php"]));
        map.insert("php4", TagSet::from([tags::TEXT, "php"]));
        map.insert("php5", TagSet::from([tags::TEXT, "php"]));
        map.insert("phtml", TagSet::from([tags::TEXT, "php"]));
        map.insert("pl", TagSet::from([tags::TEXT, "perl"]));
        map.insert("plantuml", TagSet::from([tags::TEXT, "plantuml"]));
        map.insert("pm", TagSet::from([tags::TEXT, "perl"]));
        map.insert("png", TagSet::from([tags::BINARY, "image", "png"]));
        map.insert("po", TagSet::from([tags::TEXT, "pofile"]));
        map.insert("pom", TagSet::from(["pom", tags::TEXT, "xml"]));
        map.insert("pp", TagSet::from([tags::TEXT, "puppet"]));
        map.insert("prisma", TagSet::from([tags::TEXT, "prisma"]));
        map.insert("properties", TagSet::from([tags::TEXT, "java-properties"]));
        map.insert("proto", TagSet::from([tags::TEXT, "proto"]));
        map.insert("ps1", TagSet::from([tags::TEXT, "powershell"]));
        map.insert("pug", TagSet::from([tags::TEXT, "pug"]));
        map.insert("puml", TagSet::from([tags::TEXT, "plantuml"]));
        map.insert("purs", TagSet::from([tags::TEXT, "purescript"]));
        map.insert("pxd", TagSet::from([tags::TEXT, "cython"]));
        map.insert("pxi", TagSet::from([tags::TEXT, "cython"]));
        map.insert("py", TagSet::from([tags::TEXT, "python"]));
        map.insert("pyi", TagSet::from([tags::TEXT, "pyi"]));
        map.insert("pyproj", TagSet::from([tags::TEXT, "xml", "pyproj"]));
        map.insert("pyt", TagSet::from([tags::TEXT, "python"]));
        map.insert("pyx", TagSet::from([tags::TEXT, "cython"]));
        map.insert("pyz", TagSet::from([tags::BINARY, "pyz"]));
        map.insert("pyzw", TagSet::from([tags::BINARY, "pyz"]));
        map.insert("qml", TagSet::from([tags::TEXT, "qml"]));
        map.insert("r", TagSet::from([tags::TEXT, "r"]));
        map.insert("rake", TagSet::from([tags::TEXT, "ruby"]));
        map.insert("rb", TagSet::from([tags::TEXT, "ruby"]));
        map.insert("resx", TagSet::from([tags::TEXT, "resx", "xml"]));
        map.insert("rng", TagSet::from([tags::TEXT, "xml", "relax-ng"]));
        map.insert("rs", TagSet::from([tags::TEXT, "rust"]));
        map.insert("rst", TagSet::from([tags::TEXT, "rst"]));
        map.insert("s", TagSet::from([tags::TEXT, "asm"]));
        map.insert("sass", TagSet::from([tags::TEXT, "sass"]));
        map.insert("sbt", TagSet::from([tags::TEXT, "sbt", "scala"]));
        map.insert("sc", TagSet::from([tags::TEXT, "scala"]));
        map.insert("scala", TagSet::from([tags::TEXT, "scala"]));
        map.insert("scm", TagSet::from([tags::TEXT, "scheme"]));
        map.insert("scss", TagSet::from([tags::TEXT, "scss"]));
        map.insert("sh", TagSet::from([tags::TEXT, "shell"]));
        map.insert("sln", TagSet::from([tags::TEXT, "sln"]));
        map.insert("sls", TagSet::from([tags::TEXT, "salt"]));
        map.insert("so", TagSet::from([tags::BINARY]));
        map.insert("sol", TagSet::from([tags::TEXT, "solidity"]));
        map.insert("spec", TagSet::from([tags::TEXT, "spec"]));
        map.insert("sql", TagSet::from([tags::TEXT, "sql"]));
        map.insert("ss", TagSet::from([tags::TEXT, "scheme"]));
        map.insert("sty", TagSet::from([tags::TEXT, "tex"]));
        map.insert("styl", TagSet::from([tags::TEXT, "stylus"]));
        map.insert("sv", TagSet::from([tags::TEXT, "system-verilog"]));
        map.insert("svelte", TagSet::from([tags::TEXT, "svelte"]));
        map.insert("svg", TagSet::from([tags::TEXT, "image", "svg", "xml"]));
        map.insert("svh", TagSet::from([tags::TEXT, "system-verilog"]));
        map.insert("swf", TagSet::from([tags::BINARY, "swf"]));
        map.insert("swift", TagSet::from([tags::TEXT, "swift"]));
        map.insert("swiftdeps", TagSet::from([tags::TEXT, "swiftdeps"]));
        map.insert("tac", TagSet::from([tags::TEXT, "twisted", "python"]));
        map.insert("tar", TagSet::from([tags::BINARY, "tar"]));
        map.insert("tex", TagSet::from([tags::TEXT, "tex"]));
        map.insert("textproto", TagSet::from([tags::TEXT, "textproto"]));
        map.insert("tf", TagSet::from([tags::TEXT, "terraform"]));
        map.insert("tfvars", TagSet::from([tags::TEXT, "terraform"]));
        map.insert("tgz", TagSet::from([tags::BINARY, "gzip"]));
        map.insert("thrift", TagSet::from([tags::TEXT, "thrift"]));
        map.insert("tiff", TagSet::from([tags::BINARY, "image", "tiff"]));
        map.insert("toml", TagSet::from([tags::TEXT, "toml"]));
        map.insert("ts", TagSet::from([tags::TEXT, "ts"]));
        map.insert("tsv", TagSet::from([tags::TEXT, "tsv"]));
        map.insert("tsx", TagSet::from([tags::TEXT, "tsx"]));
        map.insert("ttf", TagSet::from([tags::BINARY, "ttf"]));
        map.insert("twig", TagSet::from([tags::TEXT, "twig"]));
        map.insert(
            "txsprofile",
            TagSet::from([tags::TEXT, "ini", "txsprofile"]),
        );
        map.insert("txt", TagSet::from([tags::TEXT, "plain-text"]));
        map.insert("txtpb", TagSet::from([tags::TEXT, "textproto"]));
        map.insert("urdf", TagSet::from([tags::TEXT, "xml", "urdf"]));
        map.insert("v", TagSet::from([tags::TEXT, "verilog"]));
        map.insert("vb", TagSet::from([tags::TEXT, "vb"]));
        map.insert("vbproj", TagSet::from([tags::TEXT, "xml", "vbproj"]));
        map.insert("vcxproj", TagSet::from([tags::TEXT, "xml", "vcxproj"]));
        map.insert("vdx", TagSet::from([tags::TEXT, "vdx"]));
        map.insert("vh", TagSet::from([tags::TEXT, "verilog"]));
        map.insert("vhd", TagSet::from([tags::TEXT, "vhdl"]));
        map.insert("vim", TagSet::from([tags::TEXT, "vim"]));
        map.insert("vtl", TagSet::from([tags::TEXT, "vtl"]));
        map.insert("vue", TagSet::from([tags::TEXT, "vue"]));
        map.insert("war", TagSet::from([tags::BINARY, "zip", "jar"]));
        map.insert("wav", TagSet::from([tags::BINARY, "audio", "wav"]));
        map.insert("webp", TagSet::from([tags::BINARY, "image", "webp"]));
        map.insert("whl", TagSet::from([tags::BINARY, "wheel", "zip"]));
        map.insert("wkt", TagSet::from([tags::TEXT, "wkt"]));
        map.insert("woff", TagSet::from([tags::BINARY, "woff"]));
        map.insert("woff2", TagSet::from([tags::BINARY, "woff2"]));
        map.insert("wsgi", TagSet::from([tags::TEXT, "wsgi", "python"]));
        map.insert("xacro", TagSet::from([tags::TEXT, "xml", "urdf", "xacro"]));
        map.insert("xctestplan", TagSet::from([tags::TEXT, "json"]));
        map.insert("xhtml", TagSet::from([tags::TEXT, "xml", "html", "xhtml"]));
        map.insert("xml", TagSet::from([tags::TEXT, "xml"]));
        map.insert("xq", TagSet::from([tags::TEXT, "xquery"]));
        map.insert("xql", TagSet::from([tags::TEXT, "xquery"]));
        map.insert("xqm", TagSet::from([tags::TEXT, "xquery"]));
        map.insert("xqu", TagSet::from([tags::TEXT, "xquery"]));
        map.insert("xquery", TagSet::from([tags::TEXT, "xquery"]));
        map.insert("xqy", TagSet::from([tags::TEXT, "xquery"]));
        map.insert("xsd", TagSet::from([tags::TEXT, "xml", "xsd"]));
        map.insert("xsl", TagSet::from([tags::TEXT, "xml", "xsl"]));
        map.insert("yaml", TagSet::from([tags::TEXT, "yaml"]));
        map.insert("yamlld", TagSet::from([tags::TEXT, "yaml", "yamlld"]));
        map.insert("yang", TagSet::from([tags::TEXT, "yang"]));
        map.insert("yin", TagSet::from([tags::TEXT, "xml", "yin"]));
        map.insert("yml", TagSet::from([tags::TEXT, "yaml"]));
        map.insert("zcml", TagSet::from([tags::TEXT, "xml", "zcml"]));
        map.insert("zig", TagSet::from([tags::TEXT, "zig"]));
        map.insert("zip", TagSet::from([tags::BINARY, "zip"]));
        map.insert("zpt", TagSet::from([tags::TEXT, "zpt"]));
        map.insert("zsh", TagSet::from([tags::TEXT, "shell", "zsh"]));
        map.insert("plist", TagSet::from(["plist"]));
        map.insert("ppm", TagSet::from(["image", "ppm"]));

        map
    })
}

fn by_filename() -> &'static TagMap {
    static FILENAMES: OnceLock<TagMap> = OnceLock::new();
    FILENAMES.get_or_init(|| {
        let extensions = by_extension();
        let mut map = TagMap::default();

        map.insert(".ansible-lint", extensions.clone_key("yaml"));
        map.insert(
            ".babelrc",
            extensions.clone_key("json").with_added(&["babelrc"]),
        );
        map.insert(".bash_aliases", extensions.clone_key("bash"));
        map.insert(".bash_profile", extensions.clone_key("bash"));
        map.insert(".bashrc", extensions.clone_key("bash"));
        map.insert(".bazelrc", TagSet::from([tags::TEXT, "bazelrc"]));
        map.insert(
            ".bowerrc",
            extensions.clone_key("json").with_added(&["bowerrc"]),
        );
        map.insert(
            ".browserslistrc",
            TagSet::from([tags::TEXT, "browserslistrc"]),
        );
        map.insert(".clang-format", extensions.clone_key("yaml"));
        map.insert(".clang-tidy", extensions.clone_key("yaml"));
        map.insert(
            ".codespellrc",
            extensions.clone_key("ini").with_added(&["codespellrc"]),
        );
        map.insert(
            ".coveragerc",
            extensions.clone_key("ini").with_added(&["coveragerc"]),
        );
        map.insert(".cshrc", extensions.clone_key("csh"));
        map.insert(
            ".csslintrc",
            extensions.clone_key("json").with_added(&["csslintrc"]),
        );
        map.insert(".dockerignore", TagSet::from([tags::TEXT, "dockerignore"]));
        map.insert(".editorconfig", TagSet::from([tags::TEXT, "editorconfig"]));
        map.insert(
            ".flake8",
            extensions.clone_key("ini").with_added(&["flake8"]),
        );
        map.insert(
            ".gitattributes",
            TagSet::from([tags::TEXT, "gitattributes"]),
        );
        map.insert(
            ".gitconfig",
            extensions.clone_key("ini").with_added(&["gitconfig"]),
        );
        map.insert(".gitignore", TagSet::from([tags::TEXT, "gitignore"]));
        map.insert(
            ".gitlint",
            extensions.clone_key("ini").with_added(&["gitlint"]),
        );
        map.insert(".gitmodules", TagSet::from([tags::TEXT, "gitmodules"]));
        map.insert(".hgrc", extensions.clone_key("ini").with_added(&["hgrc"]));
        map.insert(
            ".isort.cfg",
            extensions.clone_key("ini").with_added(&["isort"]),
        );
        map.insert(
            ".jshintrc",
            extensions.clone_key("json").with_added(&["jshintrc"]),
        );
        map.insert(".mailmap", TagSet::from([tags::TEXT, "mailmap"]));
        map.insert(
            ".mention-bot",
            extensions.clone_key("json").with_added(&["mention-bot"]),
        );
        map.insert(".npmignore", TagSet::from([tags::TEXT, "npmignore"]));
        map.insert(".pdbrc", extensions.clone_key("py").with_added(&["pdbrc"]));
        map.insert(
            ".prettierignore",
            TagSet::from([tags::TEXT, "gitignore", "prettierignore"]),
        );
        map.insert(
            ".pypirc",
            extensions.clone_key("ini").with_added(&["pypirc"]),
        );
        map.insert(".rstcheck.cfg", extensions.clone_key("ini"));
        map.insert(
            ".salt-lint",
            extensions.clone_key("yaml").with_added(&["salt-lint"]),
        );
        map.insert(
            ".yamllint",
            extensions.clone_key("yaml").with_added(&["yamllint"]),
        );
        map.insert(".zlogin", extensions.clone_key("zsh"));
        map.insert(".zlogout", extensions.clone_key("zsh"));
        map.insert(".zprofile", extensions.clone_key("zsh"));
        map.insert(".zshrc", extensions.clone_key("zsh"));
        map.insert(".zshenv", extensions.clone_key("zsh"));

        map.insert("AUTHORS", extensions.clone_key("txt"));
        map.insert("BUILD", extensions.clone_key("bzl"));
        map.insert(
            "Cargo.toml",
            extensions.clone_key("toml").with_added(&["cargo"]),
        );
        map.insert(
            "Cargo.lock",
            extensions.clone_key("toml").with_added(&["cargo-lock"]),
        );
        map.insert("CMakeLists.txt", extensions.clone_key("cmake"));
        map.insert("CHANGELOG", extensions.clone_key("txt"));
        map.insert("config.ru", extensions.clone_key("rb"));
        map.insert("Containerfile", TagSet::from([tags::TEXT, "dockerfile"]));
        map.insert("CONTRIBUTING", extensions.clone_key("txt"));
        map.insert("copy.bara.sky", extensions.clone_key("bzl"));
        map.insert("COPYING", extensions.clone_key("txt"));
        map.insert("Dockerfile", TagSet::from([tags::TEXT, "dockerfile"]));
        map.insert("Gemfile", extensions.clone_key("rb"));
        map.insert("Gemfile.lock", TagSet::from([tags::TEXT]));
        map.insert("GNUmakefile", extensions.clone_key("mk"));
        map.insert("go.mod", TagSet::from([tags::TEXT, "go-mod"]));
        map.insert("go.sum", TagSet::from([tags::TEXT, "go-sum"]));
        map.insert("Jenkinsfile", extensions.clone_key("jenkins"));
        map.insert("LICENSE", extensions.clone_key("txt"));
        map.insert("MAINTAINERS", extensions.clone_key("txt"));
        map.insert("Makefile", extensions.clone_key("mk"));
        map.insert("meson.build", extensions.clone_key("meson"));
        map.insert("meson_options.txt", extensions.clone_key("meson"));
        map.insert("makefile", extensions.clone_key("mk"));
        map.insert("NEWS", extensions.clone_key("txt"));
        map.insert("NOTICE", extensions.clone_key("txt"));
        map.insert("PATENTS", extensions.clone_key("txt"));
        map.insert("Pipfile", extensions.clone_key("toml"));
        map.insert("Pipfile.lock", extensions.clone_key("json"));
        map.insert(
            "PKGBUILD",
            TagSet::from([tags::TEXT, "bash", "pkgbuild", "alpm"]),
        );
        map.insert("poetry.lock", extensions.clone_key("toml"));
        map.insert("pom.xml", extensions.clone_key("pom"));
        map.insert(
            "pylintrc",
            extensions.clone_key("ini").with_added(&["pylintrc"]),
        );
        map.insert("README", extensions.clone_key("txt"));
        map.insert("Rakefile", extensions.clone_key("rb"));
        map.insert("rebar.config", extensions.clone_key("erl"));
        map.insert("setup.cfg", extensions.clone_key("ini"));
        map.insert("sys.config", extensions.clone_key("erl"));
        map.insert("sys.config.src", extensions.clone_key("erl"));
        map.insert("Vagrantfile", extensions.clone_key("rb"));
        map.insert("WORKSPACE", extensions.clone_key("bzl"));
        map.insert("wscript", extensions.clone_key("py"));

        map
    })
}

fn by_interpreter() -> &'static TagMap {
    static INTERPRETERS: OnceLock<TagMap> = OnceLock::new();
    INTERPRETERS.get_or_init(|| {
        let mut map = TagMap::default();
        map.insert("ash", TagSet::from(["shell", "ash"]));
        map.insert("awk", TagSet::from(["awk"]));
        map.insert("bash", TagSet::from(["shell", "bash"]));
        map.insert("bats", TagSet::from(["shell", "bash", "bats"]));
        map.insert("cbsd", TagSet::from(["shell", "cbsd"]));
        map.insert("csh", TagSet::from(["shell", "csh"]));
        map.insert("dash", TagSet::from(["shell", "dash"]));
        map.insert("expect", TagSet::from(["expect"]));
        map.insert("ksh", TagSet::from(["shell", "ksh"]));
        map.insert("node", TagSet::from(["javascript"]));
        map.insert("nodejs", TagSet::from(["javascript"]));
        map.insert("perl", TagSet::from(["perl"]));
        map.insert("php", TagSet::from(["php"]));
        map.insert("php7", TagSet::from(["php", "php7"]));
        map.insert("php8", TagSet::from(["php", "php8"]));
        map.insert("python", TagSet::from(["python"]));
        map.insert("python2", TagSet::from(["python", "python2"]));
        map.insert("python3", TagSet::from(["python", "python3"]));
        map.insert("ruby", TagSet::from(["ruby"]));
        map.insert("sh", TagSet::from(["shell", "sh"]));
        map.insert("tcsh", TagSet::from(["shell", "tcsh"]));
        map.insert("zsh", TagSet::from(["shell", "zsh"]));
        map
    })
}

fn is_type_tag(tag: &str) -> bool {
    matches!(
        tag,
        tags::DIRECTORY | tags::SYMLINK | tags::SOCKET | tags::FILE
    )
}

fn is_mode_tag(tag: &str) -> bool {
    matches!(tag, tags::EXECUTABLE | tags::NON_EXECUTABLE)
}

fn is_encoding_tag(tag: &str) -> bool {
    matches!(tag, tags::TEXT | tags::BINARY)
}

/// Identify tags for a file at the given path.
pub(crate) fn tags_from_path(path: &Path) -> Result<TagSet> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        return Ok(TagSet::from([tags::DIRECTORY]));
    } else if metadata.is_symlink() {
        return Ok(TagSet::from([tags::SYMLINK]));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let file_type = metadata.file_type();
        if file_type.is_socket() {
            return Ok(TagSet::from([tags::SOCKET]));
        } else if file_type.is_fifo() {
            return Ok(TagSet::from([tags::FIFO]));
        } else if file_type.is_block_device() {
            return Ok(TagSet::from([tags::BLOCK_DEVICE]));
        } else if file_type.is_char_device() {
            return Ok(TagSet::from([tags::CHARACTER_DEVICE]));
        }
    };

    let mut tags = TagSet::new();
    tags.insert(tags::FILE);

    let executable;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        executable = metadata.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    {
        // `pre-commit/identify` uses `os.access(path, os.X_OK)` to check for executability on Windows.
        // This would actually return true for any file.
        // We keep this behavior for compatibility.
        executable = true;
    }

    if executable {
        tags.insert(tags::EXECUTABLE);
    } else {
        tags.insert(tags::NON_EXECUTABLE);
    }

    let filename_tags = tags_from_filename(path);
    tags.extend(filename_tags.iter());
    if executable {
        if let Ok(shebang) = parse_shebang(path) {
            let interpreter_tags = tags_from_interpreter(shebang[0].as_str());
            tags.extend(interpreter_tags.iter());
        }
    }

    if !tags.iter().any(is_encoding_tag) {
        if is_text_file(path) {
            tags.insert(tags::TEXT);
        } else {
            tags.insert(tags::BINARY);
        }
    }

    Ok(tags)
}

fn tags_from_filename(filename: &Path) -> TagSet {
    let ext = filename.extension().and_then(|ext| ext.to_str());
    let filename = filename
        .file_name()
        .and_then(|name| name.to_str())
        .expect("Invalid filename");

    let mut result = TagSet::new();

    if let Some(tags) = by_filename().get(filename) {
        result.extend(tags.iter());
    }
    if result.is_empty() {
        // # Allow e.g. "Dockerfile.xenial" to match "Dockerfile".
        if let Some(name) = filename.split('.').next() {
            if let Some(tags) = by_filename().get(name) {
                result.extend(tags.iter());
            }
        }
    }

    if let Some(ext) = ext {
        // Check if extension is already lowercase to avoid allocation
        if ext.chars().all(|c| c.is_ascii_lowercase()) {
            if let Some(tags) = by_extension().get(ext) {
                result.extend(tags.iter());
            }
        } else {
            let ext_lower = ext.to_ascii_lowercase();
            if let Some(tags) = by_extension().get(ext_lower.as_str()) {
                result.extend(tags.iter());
            }
        }
    }

    result
}

fn tags_from_interpreter(interpreter: &str) -> TagSet {
    let mut name = interpreter
        .rfind('/')
        .map(|pos| &interpreter[pos + 1..])
        .unwrap_or(interpreter);

    while !name.is_empty() {
        if let Some(tags) = by_interpreter().get(name) {
            return tags.clone();
        }

        // python3.12.3 should match python3.12.3, python3.12, python3, python
        if let Some(pos) = name.rfind('.') {
            name = &name[..pos];
        } else {
            break;
        }
    }

    TagSet::new()
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum ShebangError {
    #[error("No shebang found")]
    NoShebang,
    #[error("Shebang contains non-printable characters")]
    NonPrintableChars,
    #[error("Failed to parse shebang")]
    ParseFailed,
    #[error("No command found in shebang")]
    NoCommand,
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

fn starts_with(slice: &[String], prefix: &[&str]) -> bool {
    slice.len() >= prefix.len() && slice.iter().zip(prefix.iter()).all(|(s, p)| s == p)
}

/// Parse nix-shell shebangs, which may span multiple lines.
/// See: <https://nixos.wiki/wiki/Nix-shell_shebang>
/// Example:
/// `#!nix-shell -i python3 -p python3` would return `["python3"]`
fn parse_nix_shebang<R: BufRead>(reader: &mut R, mut cmd: Vec<String>) -> Vec<String> {
    loop {
        let Ok(buf) = reader.fill_buf() else {
            break;
        };

        if buf.len() < 2 || &buf[..2] != b"#!" {
            break;
        }

        reader.consume(2);

        let mut next_line = String::new();
        match reader.read_line(&mut next_line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                if err.kind() == std::io::ErrorKind::InvalidData {
                    return cmd;
                }
                break;
            }
        }

        let trimmed = next_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(line_tokens) = shlex::split(trimmed) {
            for idx in 0..line_tokens.len().saturating_sub(1) {
                if line_tokens[idx] == "-i" {
                    if let Some(interpreter) = line_tokens.get(idx + 1) {
                        cmd = vec![interpreter.clone()];
                    }
                }
            }
        }
    }

    cmd
}

pub(crate) fn parse_shebang(path: &Path) -> Result<Vec<String>, ShebangError> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if !line.starts_with("#!") {
        return Err(ShebangError::NoShebang);
    }

    // Require only printable ASCII
    if line
        .bytes()
        .any(|b| !(0x20..=0x7E).contains(&b) && !(0x09..=0x0D).contains(&b))
    {
        return Err(ShebangError::NonPrintableChars);
    }

    let mut tokens = shlex::split(line[2..].trim()).ok_or(ShebangError::ParseFailed)?;
    let mut cmd =
        if starts_with(&tokens, &["/usr/bin/env", "-S"]) || starts_with(&tokens, &["env", "-S"]) {
            tokens.drain(0..2);
            tokens
        } else if starts_with(&tokens, &["/usr/bin/env"]) || starts_with(&tokens, &["env"]) {
            tokens.drain(0..1);
            tokens
        } else {
            tokens
        };
    if cmd.is_empty() {
        return Err(ShebangError::NoCommand);
    }
    if cmd[0] == "nix-shell" {
        cmd = parse_nix_shebang(&mut reader, cmd);
    }
    if cmd.is_empty() {
        return Err(ShebangError::NoCommand);
    }

    Ok(cmd)
}

// Lookup table for text character detection.
static IS_TEXT_CHAR: [u32; 8] = {
    let mut table = [0u32; 8];
    let mut i = 0;
    while i < 256 {
        // Printable ASCII (0x20..0x7F)
        // High bit set (>= 0x80)
        // Control characters: 7, 8, 9, 10, 11, 12, 13, 27
        let is_text =
            (i >= 0x20 && i < 0x7F) || i >= 0x80 || matches!(i, 7 | 8 | 9 | 10 | 11 | 12 | 13 | 27);
        if is_text {
            table[i / 32] |= 1 << (i % 32);
        }
        i += 1;
    }
    table
};

fn is_text_char(b: u8) -> bool {
    let idx = b as usize;
    (IS_TEXT_CHAR[idx / 32] & (1 << (idx % 32))) != 0
}

/// Return whether the first KB of contents seems to be binary.
///
/// This is roughly based on libmagic's binary/text detection:
/// <https://github.com/file/file/blob/df74b09b9027676088c797528edcaae5a9ce9ad0/src/encoding.c#L203-L228>
fn is_text_file(path: &Path) -> bool {
    let mut buffer = [0; 1024];
    let Ok(mut file) = fs_err::File::open(path) else {
        return false;
    };

    let Ok(bytes_read) = file.read(&mut buffer) else {
        return false;
    };
    if bytes_read == 0 {
        return true;
    }

    buffer[..bytes_read].iter().all(|&b| is_text_char(b))
}

pub fn all_tags() -> &'static FxHashSet<&'static str> {
    static ALL_TAGS: OnceLock<FxHashSet<&'static str>> = OnceLock::new();
    ALL_TAGS.get_or_init(|| {
        let mut tags_set = FxHashSet::default();

        tags_set.insert(tags::DIRECTORY);
        tags_set.insert(tags::SYMLINK);
        tags_set.insert(tags::SOCKET);
        tags_set.insert(tags::FIFO);
        tags_set.insert(tags::BLOCK_DEVICE);
        tags_set.insert(tags::CHARACTER_DEVICE);
        tags_set.insert(tags::FILE);
        tags_set.insert(tags::EXECUTABLE);
        tags_set.insert(tags::NON_EXECUTABLE);
        tags_set.insert(tags::TEXT);
        tags_set.insert(tags::BINARY);

        for tags in by_extension().values() {
            for tag in tags.iter() {
                tags_set.insert(tag);
            }
        }

        for tags in by_filename().values() {
            for tag in tags.iter() {
                tags_set.insert(tag);
            }
        }

        for tags in by_interpreter().values() {
            for tag in tags.iter() {
                tags_set.insert(tag);
            }
        }

        tags_set
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    fn assert_tagset(actual: &TagSet, expected: &[&'static str]) {
        let mut actual_vec: Vec<_> = actual.iter().collect();
        actual_vec.sort_unstable();
        let mut expected_vec = expected.to_vec();
        expected_vec.sort_unstable();
        assert_eq!(actual_vec, expected_vec);
    }

    #[test]
    #[cfg(unix)]
    fn tags_from_path() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("source.txt");
        let dest = dir.path().join("link.txt");
        fs_err::File::create(&src)?;
        std::os::unix::fs::symlink(&src, &dest)?;

        let tags = super::tags_from_path(dir.path())?;
        assert_tagset(&tags, &["directory"]);
        let tags = super::tags_from_path(&src)?;
        assert_tagset(&tags, &["plain-text", "non-executable", "file", "text"]);
        let tags = super::tags_from_path(&dest)?;
        assert_tagset(&tags, &["symlink"]);

        Ok(())
    }

    #[test]
    #[cfg(windows)]
    fn tags_from_path() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("source.txt");
        fs_err::File::create(&src)?;

        let tags = super::tags_from_path(dir.path())?;
        assert_tagset(&tags, &["directory"]);
        let tags = super::tags_from_path(&src)?;
        assert_tagset(&tags, &["plain-text", "executable", "file", "text"]);

        Ok(())
    }

    #[test]
    fn tags_from_filename() {
        let tags = super::tags_from_filename(Path::new("test.py"));
        assert_tagset(&tags, &["python", "text"]);

        let tags = super::tags_from_filename(Path::new("data.json"));
        assert_tagset(&tags, &["json", "text"]);

        let tags = super::tags_from_filename(Path::new("Pipfile"));
        assert_tagset(&tags, &["toml", "text"]);

        let tags = super::tags_from_filename(Path::new("Pipfile.lock"));
        assert_tagset(&tags, &["json", "text"]);

        let tags = super::tags_from_filename(Path::new("file.pdf"));
        assert_tagset(&tags, &["pdf", "binary"]);

        let tags = super::tags_from_filename(Path::new("FILE.PDF"));
        assert_tagset(&tags, &["pdf", "binary"]);
    }

    #[test]
    fn tags_from_interpreter() {
        let tags = super::tags_from_interpreter("/usr/bin/python3");
        assert_tagset(&tags, &["python", "python3"]);

        let tags = super::tags_from_interpreter("/usr/bin/python3.12");
        assert_tagset(&tags, &["python", "python3"]);

        let tags = super::tags_from_interpreter("/usr/bin/python3.12.3");
        assert_tagset(&tags, &["python", "python3"]);

        let tags = super::tags_from_interpreter("python");
        assert_tagset(&tags, &["python"]);

        let tags = super::tags_from_interpreter("sh");
        assert_tagset(&tags, &["shell", "sh"]);

        let tags = super::tags_from_interpreter("invalid");
        assert!(tags.is_empty());
    }

    #[test]
    fn parse_shebang_nix_shell_interpreter() -> anyhow::Result<()> {
        let mut file = tempfile::NamedTempFile::new()?;
        writeln!(
            file,
            indoc::indoc! {r#"
            #!/usr/bin/env nix-shell
            #! nix-shell --pure -i bash -p "python3.withPackages (p: [ p.numpy p.sympy ])"
            #! nix-shell -I nixpkgs=https://example.com
            echo hi
            "#}
        )?;
        file.flush()?;

        let cmd = super::parse_shebang(file.path())?;
        assert_eq!(cmd, vec!["bash"]);

        Ok(())
    }

    #[test]
    fn parse_shebang_nix_shell_without_interpreter() -> anyhow::Result<()> {
        let mut file = tempfile::NamedTempFile::new()?;
        writeln!(
            file,
            indoc::indoc! {r"
            #!/usr/bin/env nix-shell -p python3
            #! nix-shell --pure -I nixpkgs=https://example.com
            echo hi
            "}
        )?;
        file.flush()?;

        let cmd = super::parse_shebang(file.path())?;
        assert_eq!(cmd, vec!["nix-shell", "-p", "python3"]);

        Ok(())
    }
}
