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
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::OnceLock;
use std::vec;

use anyhow::Result;
use rustc_hash::{FxHashMap, FxHashSet};

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

fn by_extension() -> &'static FxHashMap<&'static str, Vec<&'static str>> {
    static EXTENSIONS: OnceLock<FxHashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    EXTENSIONS.get_or_init(|| {
        let mut map = FxHashMap::default();
        map.insert("adoc", vec!["text", "asciidoc"]);
        map.insert("ai", vec!["binary", "adobe-illustrator"]);
        map.insert("aj", vec!["text", "aspectj"]);
        map.insert("asciidoc", vec!["text", "asciidoc"]);
        map.insert("apinotes", vec!["text", "apinotes"]);
        map.insert("asar", vec!["binary", "asar"]);
        map.insert("astro", vec!["text", "astro"]);
        map.insert("avif", vec!["binary", "image", "avif"]);
        map.insert("avsc", vec!["text", "avro-schema"]);
        map.insert("bash", vec!["text", "shell", "bash"]);
        map.insert("bat", vec!["text", "batch"]);
        map.insert("bats", vec!["text", "shell", "bash", "bats"]);
        map.insert("bazel", vec!["text", "bazel"]);
        map.insert("beancount", vec!["text", "beancount"]);
        map.insert("bib", vec!["text", "bib"]);
        map.insert("bmp", vec!["binary", "image", "bitmap"]);
        map.insert("bz2", vec!["binary", "bzip2"]);
        map.insert("bzl", vec!["text", "bazel"]);
        map.insert("c", vec!["text", "c"]);
        map.insert("c++", vec!["text", "c++"]);
        map.insert("c++m", vec!["text", "c++"]);
        map.insert("cc", vec!["text", "c++"]);
        map.insert("ccm", vec!["text", "c++"]);
        map.insert("cfg", vec!["text"]);
        map.insert("chs", vec!["text", "c2hs"]);
        map.insert("cjs", vec!["text", "javascript"]);
        map.insert("clj", vec!["text", "clojure"]);
        map.insert("cljc", vec!["text", "clojure"]);
        map.insert("cljs", vec!["text", "clojure", "clojurescript"]);
        map.insert("cmake", vec!["text", "cmake"]);
        map.insert("cnf", vec!["text"]);
        map.insert("coffee", vec!["text", "coffee"]);
        map.insert("conf", vec!["text"]);
        map.insert("cpp", vec!["text", "c++"]);
        map.insert("cppm", vec!["text", "c++"]);
        map.insert("cr", vec!["text", "crystal"]);
        map.insert("crt", vec!["text", "pem"]);
        map.insert("cs", vec!["text", "c#"]);
        map.insert("csproj", vec!["text", "xml", "csproj"]);
        map.insert("csh", vec!["text", "shell", "csh"]);
        map.insert("cson", vec!["text", "cson"]);
        map.insert("css", vec!["text", "css"]);
        map.insert("csv", vec!["text", "csv"]);
        map.insert("cu", vec!["text", "cuda"]);
        map.insert("cue", vec!["text", "cue"]);
        map.insert("cuh", vec!["text", "cuda"]);
        map.insert("cxx", vec!["text", "c++"]);
        map.insert("cxxm", vec!["text", "c++"]);
        map.insert("cylc", vec!["text", "cylc"]);
        map.insert("dart", vec!["text", "dart"]);
        map.insert("dbc", vec!["text", "dbc"]);
        map.insert("def", vec!["text", "def"]);
        map.insert("dll", vec!["binary"]);
        map.insert("dtd", vec!["text", "dtd"]);
        map.insert("ear", vec!["binary", "zip", "jar"]);
        map.insert("edn", vec!["text", "clojure", "edn"]);
        map.insert("ejs", vec!["text", "ejs"]);
        map.insert("ejson", vec!["text", "json", "ejson"]);
        map.insert("env", vec!["text", "dotenv"]);
        map.insert("eot", vec!["binary", "eot"]);
        map.insert("eps", vec!["binary", "eps"]);
        map.insert("erb", vec!["text", "erb"]);
        map.insert("erl", vec!["text", "erlang"]);
        map.insert("ex", vec!["text", "elixir"]);
        map.insert("exe", vec!["binary"]);
        map.insert("exs", vec!["text", "elixir"]);
        map.insert("eyaml", vec!["text", "yaml"]);
        map.insert("f03", vec!["text", "fortran"]);
        map.insert("f08", vec!["text", "fortran"]);
        map.insert("f90", vec!["text", "fortran"]);
        map.insert("f95", vec!["text", "fortran"]);
        map.insert("feature", vec!["text", "gherkin"]);
        map.insert("fish", vec!["text", "fish"]);
        map.insert("fits", vec!["binary", "fits"]);
        map.insert("gd", vec!["text", "gdscript"]);
        map.insert("gemspec", vec!["text", "ruby"]);
        map.insert("geojson", vec!["text", "geojson", "json"]);
        map.insert("ggb", vec!["binary", "zip", "ggb"]);
        map.insert("gif", vec!["binary", "image", "gif"]);
        map.insert("go", vec!["text", "go"]);
        map.insert("gotmpl", vec!["text", "gotmpl"]);
        map.insert("gpx", vec!["text", "gpx", "xml"]);
        map.insert("graphql", vec!["text", "graphql"]);
        map.insert("gradle", vec!["text", "groovy"]);
        map.insert("groovy", vec!["text", "groovy"]);
        map.insert("gyb", vec!["text", "gyb"]);
        map.insert("gyp", vec!["text", "gyp", "python"]);
        map.insert("gypi", vec!["text", "gyp", "python"]);
        map.insert("gz", vec!["binary", "gzip"]);
        map.insert("h", vec!["text", "header", "c", "c++"]);
        map.insert("hbs", vec!["text", "handlebars"]);
        map.insert("hcl", vec!["text", "hcl"]);
        map.insert("hh", vec!["text", "header", "c++"]);
        map.insert("hpp", vec!["text", "header", "c++"]);
        map.insert("hrl", vec!["text", "erlang"]);
        map.insert("hs", vec!["text", "haskell"]);
        map.insert("htm", vec!["text", "html"]);
        map.insert("html", vec!["text", "html"]);
        map.insert("hxx", vec!["text", "header", "c++"]);
        map.insert("icns", vec!["binary", "icns"]);
        map.insert("ico", vec!["binary", "icon"]);
        map.insert("ics", vec!["text", "icalendar"]);
        map.insert("idl", vec!["text", "idl"]);
        map.insert("idr", vec!["text", "idris"]);
        map.insert("inc", vec!["text", "inc"]);
        map.insert("ini", vec!["text", "ini"]);
        map.insert("inl", vec!["text", "inl", "c++"]);
        map.insert("ino", vec!["text", "ino", "c++"]);
        map.insert("inx", vec!["text", "xml", "inx"]);
        map.insert("ipynb", vec!["text", "jupyter", "json"]);
        map.insert("ixx", vec!["text", "c++"]);
        map.insert("j2", vec!["text", "jinja"]);
        map.insert("jade", vec!["text", "jade"]);
        map.insert("jar", vec!["binary", "zip", "jar"]);
        map.insert("java", vec!["text", "java"]);
        map.insert("jenkins", vec!["text", "groovy", "jenkins"]);
        map.insert("jenkinsfile", vec!["text", "groovy", "jenkins"]);
        map.insert("jinja", vec!["text", "jinja"]);
        map.insert("jinja2", vec!["text", "jinja"]);
        map.insert("jl", vec!["text", "julia"]);
        map.insert("jpeg", vec!["binary", "image", "jpeg"]);
        map.insert("jpg", vec!["binary", "image", "jpeg"]);
        map.insert("js", vec!["text", "javascript"]);
        map.insert("json", vec!["text", "json"]);
        map.insert("jsonld", vec!["text", "json", "jsonld"]);
        map.insert("jsonnet", vec!["text", "jsonnet"]);
        map.insert("json5", vec!["text", "json5"]);
        map.insert("jsx", vec!["text", "jsx"]);
        map.insert("key", vec!["text", "pem"]);
        map.insert("kml", vec!["text", "kml", "xml"]);
        map.insert("kt", vec!["text", "kotlin"]);
        map.insert("kts", vec!["text", "kotlin"]);
        map.insert("lean", vec!["text", "lean"]);
        map.insert("lektorproject", vec!["text", "ini", "lektorproject"]);
        map.insert("less", vec!["text", "less"]);
        map.insert("lfm", vec!["text", "lazarus", "lazarus-form"]);
        map.insert("lhs", vec!["text", "literate-haskell"]);
        map.insert("libsonnet", vec!["text", "jsonnet"]);
        map.insert("lidr", vec!["text", "idris"]);
        map.insert("liquid", vec!["text", "liquid"]);
        map.insert("lpi", vec!["text", "lazarus", "xml"]);
        map.insert("lpr", vec!["text", "lazarus", "pascal"]);
        map.insert("lr", vec!["text", "lektor"]);
        map.insert("lua", vec!["text", "lua"]);
        map.insert("m", vec!["text", "objective-c"]);
        map.insert("m4", vec!["text", "m4"]);
        map.insert("make", vec!["text", "makefile"]);
        map.insert("manifest", vec!["text", "manifest"]);
        map.insert("map", vec!["text", "map"]);
        map.insert("markdown", vec!["text", "markdown"]);
        map.insert("md", vec!["text", "markdown"]);
        map.insert("mdx", vec!["text", "mdx"]);
        map.insert("meson", vec!["text", "meson"]);
        map.insert("metal", vec!["text", "metal"]);
        map.insert("mib", vec!["text", "mib"]);
        map.insert("mjs", vec!["text", "javascript"]);
        map.insert("mk", vec!["text", "makefile"]);
        map.insert("ml", vec!["text", "ocaml"]);
        map.insert("mli", vec!["text", "ocaml"]);
        map.insert("mm", vec!["text", "c++", "objective-c++"]);
        map.insert("modulemap", vec!["text", "modulemap"]);
        map.insert("mscx", vec!["text", "xml", "musescore"]);
        map.insert("mscz", vec!["binary", "zip", "musescore"]);
        map.insert("mustache", vec!["text", "mustache"]);
        map.insert("myst", vec!["text", "myst"]);
        map.insert("ngdoc", vec!["text", "ngdoc"]);
        map.insert("nim", vec!["text", "nim"]);
        map.insert("nims", vec!["text", "nim"]);
        map.insert("nimble", vec!["text", "nimble"]);
        map.insert("nix", vec!["text", "nix"]);
        map.insert("njk", vec!["text", "nunjucks"]);
        map.insert("otf", vec!["binary", "otf"]);
        map.insert("p12", vec!["binary", "p12"]);
        map.insert("pas", vec!["text", "pascal"]);
        map.insert("patch", vec!["text", "diff"]);
        map.insert("pdf", vec!["binary", "pdf"]);
        map.insert("pem", vec!["text", "pem"]);
        map.insert("php", vec!["text", "php"]);
        map.insert("php4", vec!["text", "php"]);
        map.insert("php5", vec!["text", "php"]);
        map.insert("phtml", vec!["text", "php"]);
        map.insert("pl", vec!["text", "perl"]);
        map.insert("plantuml", vec!["text", "plantuml"]);
        map.insert("pm", vec!["text", "perl"]);
        map.insert("png", vec!["binary", "image", "png"]);
        map.insert("po", vec!["text", "pofile"]);
        map.insert("pom", vec!["pom", "text", "xml"]);
        map.insert("pp", vec!["text", "puppet"]);
        map.insert("prisma", vec!["text", "prisma"]);
        map.insert("properties", vec!["text", "java-properties"]);
        map.insert("proto", vec!["text", "proto"]);
        map.insert("ps1", vec!["text", "powershell"]);
        map.insert("pug", vec!["text", "pug"]);
        map.insert("puml", vec!["text", "plantuml"]);
        map.insert("purs", vec!["text", "purescript"]);
        map.insert("pxd", vec!["text", "cython"]);
        map.insert("pxi", vec!["text", "cython"]);
        map.insert("py", vec!["text", "python"]);
        map.insert("pyi", vec!["text", "pyi"]);
        map.insert("pyproj", vec!["text", "xml", "pyproj"]);
        map.insert("pyt", vec!["text", "python"]);
        map.insert("pyx", vec!["text", "cython"]);
        map.insert("pyz", vec!["binary", "pyz"]);
        map.insert("pyzw", vec!["binary", "pyz"]);
        map.insert("qml", vec!["text", "qml"]);
        map.insert("r", vec!["text", "r"]);
        map.insert("rake", vec!["text", "ruby"]);
        map.insert("rb", vec!["text", "ruby"]);
        map.insert("resx", vec!["text", "resx", "xml"]);
        map.insert("rng", vec!["text", "xml", "relax-ng"]);
        map.insert("rs", vec!["text", "rust"]);
        map.insert("rst", vec!["text", "rst"]);
        map.insert("s", vec!["text", "asm"]);
        map.insert("sass", vec!["text", "sass"]);
        map.insert("sbt", vec!["text", "sbt", "scala"]);
        map.insert("sc", vec!["text", "scala"]);
        map.insert("scala", vec!["text", "scala"]);
        map.insert("scm", vec!["text", "scheme"]);
        map.insert("scss", vec!["text", "scss"]);
        map.insert("sh", vec!["text", "shell"]);
        map.insert("sln", vec!["text", "sln"]);
        map.insert("sls", vec!["text", "salt"]);
        map.insert("so", vec!["binary"]);
        map.insert("sol", vec!["text", "solidity"]);
        map.insert("spec", vec!["text", "spec"]);
        map.insert("sql", vec!["text", "sql"]);
        map.insert("ss", vec!["text", "scheme"]);
        map.insert("sty", vec!["text", "tex"]);
        map.insert("styl", vec!["text", "stylus"]);
        map.insert("sv", vec!["text", "system-verilog"]);
        map.insert("svelte", vec!["text", "svelte"]);
        map.insert("svg", vec!["text", "image", "svg", "xml"]);
        map.insert("svh", vec!["text", "system-verilog"]);
        map.insert("swf", vec!["binary", "swf"]);
        map.insert("swift", vec!["text", "swift"]);
        map.insert("swiftdeps", vec!["text", "swiftdeps"]);
        map.insert("tac", vec!["text", "twisted", "python"]);
        map.insert("tar", vec!["binary", "tar"]);
        map.insert("tex", vec!["text", "tex"]);
        map.insert("textproto", vec!["text", "textproto"]);
        map.insert("tf", vec!["text", "terraform"]);
        map.insert("tfvars", vec!["text", "terraform"]);
        map.insert("tgz", vec!["binary", "gzip"]);
        map.insert("thrift", vec!["text", "thrift"]);
        map.insert("tiff", vec!["binary", "image", "tiff"]);
        map.insert("toml", vec!["text", "toml"]);
        map.insert("ts", vec!["text", "ts"]);
        map.insert("tsv", vec!["text", "tsv"]);
        map.insert("tsx", vec!["text", "tsx"]);
        map.insert("ttf", vec!["binary", "ttf"]);
        map.insert("twig", vec!["text", "twig"]);
        map.insert("txsprofile", vec!["text", "ini", "txsprofile"]);
        map.insert("txt", vec!["text", "plain-text"]);
        map.insert("txtpb", vec!["text", "textproto"]);
        map.insert("urdf", vec!["text", "xml", "urdf"]);
        map.insert("v", vec!["text", "verilog"]);
        map.insert("vb", vec!["text", "vb"]);
        map.insert("vbproj", vec!["text", "xml", "vbproj"]);
        map.insert("vcxproj", vec!["text", "xml", "vcxproj"]);
        map.insert("vdx", vec!["text", "vdx"]);
        map.insert("vh", vec!["text", "verilog"]);
        map.insert("vhd", vec!["text", "vhdl"]);
        map.insert("vim", vec!["text", "vim"]);
        map.insert("vtl", vec!["text", "vtl"]);
        map.insert("vue", vec!["text", "vue"]);
        map.insert("war", vec!["binary", "zip", "jar"]);
        map.insert("wav", vec!["binary", "audio", "wav"]);
        map.insert("webp", vec!["binary", "image", "webp"]);
        map.insert("whl", vec!["binary", "wheel", "zip"]);
        map.insert("wkt", vec!["text", "wkt"]);
        map.insert("woff", vec!["binary", "woff"]);
        map.insert("woff2", vec!["binary", "woff2"]);
        map.insert("wsgi", vec!["text", "wsgi", "python"]);
        map.insert("xhtml", vec!["text", "xml", "html", "xhtml"]);
        map.insert("xacro", vec!["text", "xml", "urdf", "xacro"]);
        map.insert("xctestplan", vec!["text", "json"]);
        map.insert("xml", vec!["text", "xml"]);
        map.insert("xq", vec!["text", "xquery"]);
        map.insert("xql", vec!["text", "xquery"]);
        map.insert("xqm", vec!["text", "xquery"]);
        map.insert("xqu", vec!["text", "xquery"]);
        map.insert("xquery", vec!["text", "xquery"]);
        map.insert("xqy", vec!["text", "xquery"]);
        map.insert("xsd", vec!["text", "xml", "xsd"]);
        map.insert("xsl", vec!["text", "xml", "xsl"]);
        map.insert("yaml", vec!["text", "yaml"]);
        map.insert("yamlld", vec!["text", "yaml", "yamlld"]);
        map.insert("yang", vec!["text", "yang"]);
        map.insert("yin", vec!["text", "xml", "yin"]);
        map.insert("yml", vec!["text", "yaml"]);
        map.insert("zcml", vec!["text", "xml", "zcml"]);
        map.insert("zig", vec!["text", "zig"]);
        map.insert("zip", vec!["binary", "zip"]);
        map.insert("zpt", vec!["text", "zpt"]);
        map.insert("zsh", vec!["text", "shell", "zsh"]);
        map.insert("plist", vec!["plist"]);
        map.insert("ppm", vec!["image", "ppm"]);
        map
    })
}

fn by_filename() -> &'static FxHashMap<&'static str, &'static [&'static str]> {
    static FILENAMES: OnceLock<FxHashMap<&'static str, &'static [&'static str]>> = OnceLock::new();
    FILENAMES.get_or_init(|| {
        let mut map = FxHashMap::<_, &'static [&'static str]>::default();
        let extensions = by_extension();

        map.insert(".ansible-lint", &extensions["yaml"]);
        map.insert(
            ".babelrc",
            Box::leak(
                extensions["json"]
                    .iter()
                    .chain(&["babelrc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(".bash_aliases", &extensions["bash"]);
        map.insert(".bash_profile", &extensions["bash"]);
        map.insert(".bashrc", &extensions["bash"]);
        map.insert(
            ".bazelrc",
            Box::leak(vec!["text", "bazelrc"].into_boxed_slice()),
        );
        map.insert(
            ".bowerrc",
            Box::leak(
                extensions["json"]
                    .iter()
                    .chain(&["bowerrc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".browserslistrc",
            Box::leak(vec!["text", "browserslistrc"].into_boxed_slice()),
        );
        map.insert(".clang-format", &extensions["yaml"]);
        map.insert(".clang-tidy", &extensions["yaml"]);
        map.insert(
            ".codespellrc",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["codespellrc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".coveragerc",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["coveragerc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(".cshrc", &extensions["csh"]);
        map.insert(
            ".csslintrc",
            Box::leak(
                extensions["json"]
                    .iter()
                    .chain(&["csslintrc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".dockerignore",
            Box::leak(vec!["text", "dockerignore"].into_boxed_slice()),
        );
        map.insert(
            ".editorconfig",
            Box::leak(vec!["text", "editorconfig"].into_boxed_slice()),
        );
        map.insert(
            ".flake8",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["flake8"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".gitattributes",
            Box::leak(vec!["text", "gitattributes"].into_boxed_slice()),
        );
        map.insert(
            ".gitconfig",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["gitconfig"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".gitignore",
            Box::leak(vec!["text", "gitignore"].into_boxed_slice()),
        );
        map.insert(
            ".gitlint",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["gitlint"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".gitmodules",
            Box::leak(vec!["text", "gitmodules"].into_boxed_slice()),
        );
        map.insert(
            ".hgrc",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["hgrc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".isort.cfg",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["isort"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".jshintrc",
            Box::leak(
                extensions["json"]
                    .iter()
                    .chain(&["jshintrc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".mailmap",
            Box::leak(vec!["text", "mailmap"].into_boxed_slice()),
        );
        map.insert(
            ".mention-bot",
            Box::leak(
                extensions["json"]
                    .iter()
                    .chain(&["mention-bot"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".npmignore",
            Box::leak(vec!["text", "npmignore"].into_boxed_slice()),
        );
        map.insert(
            ".pdbrc",
            Box::leak(
                extensions["py"]
                    .iter()
                    .chain(&["pdbrc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".prettierignore",
            Box::leak(vec!["text", "gitignore", "prettierignore"].into_boxed_slice()),
        );
        map.insert(
            ".pypirc",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["pypirc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(".rstcheck.cfg", &extensions["ini"]);
        map.insert(
            ".salt-lint",
            Box::leak(
                extensions["yaml"]
                    .iter()
                    .chain(&["salt-lint"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            ".yamllint",
            Box::leak(
                extensions["yaml"]
                    .iter()
                    .chain(&["yamllint"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(".zlogin", &extensions["zsh"]);
        map.insert(".zlogout", &extensions["zsh"]);
        map.insert(".zprofile", &extensions["zsh"]);
        map.insert(".zshrc", &extensions["zsh"]);
        map.insert(".zshenv", &extensions["zsh"]);
        map.insert("AUTHORS", &extensions["txt"]);
        map.insert("BUILD", &extensions["bzl"]);
        map.insert(
            "Cargo.toml",
            Box::leak(
                extensions["toml"]
                    .iter()
                    .chain(&["cargo"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert(
            "Cargo.lock",
            Box::leak(
                extensions["toml"]
                    .iter()
                    .chain(&["cargo-lock"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert("CMakeLists.txt", &extensions["cmake"]);
        map.insert("CHANGELOG", &extensions["txt"]);
        map.insert("config.ru", &extensions["rb"]);
        map.insert(
            "Containerfile",
            Box::leak(vec!["text", "dockerfile"].into_boxed_slice()),
        );
        map.insert("CONTRIBUTING", &extensions["txt"]);
        map.insert("copy.bara.sky", &extensions["bzl"]);
        map.insert("COPYING", &extensions["txt"]);
        map.insert(
            "Dockerfile",
            Box::leak(vec!["text", "dockerfile"].into_boxed_slice()),
        );
        map.insert("Gemfile", &extensions["rb"]);
        map.insert("Gemfile.lock", Box::leak(vec!["text"].into_boxed_slice()));
        map.insert("GNUmakefile", &extensions["mk"]);
        map.insert(
            "go.mod",
            Box::leak(vec!["text", "go-mod"].into_boxed_slice()),
        );
        map.insert(
            "go.sum",
            Box::leak(vec!["text", "go-sum"].into_boxed_slice()),
        );
        map.insert("Jenkinsfile", &extensions["jenkins"]);
        map.insert("LICENSE", &extensions["txt"]);
        map.insert("MAINTAINERS", &extensions["txt"]);
        map.insert("Makefile", &extensions["mk"]);
        map.insert("meson.build", &extensions["meson"]);
        map.insert("meson_options.txt", &extensions["meson"]);
        map.insert("makefile", &extensions["mk"]);
        map.insert("NEWS", &extensions["txt"]);
        map.insert("NOTICE", &extensions["txt"]);
        map.insert("PATENTS", &extensions["txt"]);
        map.insert("Pipfile", &extensions["toml"]);
        map.insert("Pipfile.lock", &extensions["json"]);
        map.insert(
            "PKGBUILD",
            Box::leak(vec!["text", "bash", "pkgbuild", "alpm"].into_boxed_slice()),
        );
        map.insert("poetry.lock", &extensions["toml"]);
        map.insert("pom.xml", &extensions["pom"]);
        map.insert(
            "pylintrc",
            Box::leak(
                extensions["ini"]
                    .iter()
                    .chain(&["pylintrc"])
                    .copied()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
        );
        map.insert("README", &extensions["txt"]);
        map.insert("Rakefile", &extensions["rb"]);
        map.insert("rebar.config", &extensions["erl"]);
        map.insert("setup.cfg", &extensions["ini"]);
        map.insert("sys.config", &extensions["erl"]);
        map.insert("sys.config.src", &extensions["erl"]);
        map.insert("Vagrantfile", &extensions["rb"]);
        map.insert("WORKSPACE", &extensions["bzl"]);
        map.insert("wscript", &extensions["py"]);
        map
    })
}

fn by_interpreter() -> &'static FxHashMap<&'static str, Vec<&'static str>> {
    static INTERPRETERS: OnceLock<FxHashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    INTERPRETERS.get_or_init(|| {
        let mut map = FxHashMap::default();
        map.insert("ash", vec!["shell", "ash"]);
        map.insert("awk", vec!["awk"]);
        map.insert("bash", vec!["shell", "bash"]);
        map.insert("bats", vec!["shell", "bash", "bats"]);
        map.insert("cbsd", vec!["shell", "cbsd"]);
        map.insert("csh", vec!["shell", "csh"]);
        map.insert("dash", vec!["shell", "dash"]);
        map.insert("expect", vec!["expect"]);
        map.insert("ksh", vec!["shell", "ksh"]);
        map.insert("node", vec!["javascript"]);
        map.insert("nodejs", vec!["javascript"]);
        map.insert("perl", vec!["perl"]);
        map.insert("php", vec!["php"]);
        map.insert("php7", vec!["php", "php7"]);
        map.insert("php8", vec!["php", "php8"]);
        map.insert("python", vec!["python"]);
        map.insert("python2", vec!["python", "python2"]);
        map.insert("python3", vec!["python", "python3"]);
        map.insert("ruby", vec!["ruby"]);
        map.insert("sh", vec!["shell", "sh"]);
        map.insert("tcsh", vec!["shell", "tcsh"]);
        map.insert("zsh", vec!["shell", "zsh"]);
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

pub fn tags_from_path(path: &Path) -> Result<Vec<&str>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        return Ok(vec![tags::DIRECTORY]);
    } else if metadata.is_symlink() {
        return Ok(vec![tags::SYMLINK]);
    }
    #[cfg(unix)]
    {
        let file_type = metadata.file_type();
        if file_type.is_socket() {
            return Ok(vec![tags::SOCKET]);
        } else if file_type.is_fifo() {
            return Ok(vec![tags::FIFO]);
        } else if file_type.is_block_device() {
            return Ok(vec![tags::BLOCK_DEVICE]);
        } else if file_type.is_char_device() {
            return Ok(vec![tags::CHARACTER_DEVICE]);
        }
    };

    let mut tags = FxHashSet::default();
    tags.insert(tags::FILE);

    #[cfg(unix)]
    let executable = metadata.permissions().mode() & 0o111 != 0;
    #[cfg(not(unix))]
    let executable = {
        let ext = path.extension().and_then(|ext| ext.to_str());
        ext.is_some_and(|ext| ext == "exe" || ext == "bat" || ext == "cmd")
    };

    if executable {
        tags.insert(tags::EXECUTABLE);
    } else {
        tags.insert(tags::NON_EXECUTABLE);
    }

    tags.extend(tags_from_filename(path));
    if executable {
        if let Ok(shebang) = parse_shebang(path) {
            tags.extend(tags_from_interpreter(&shebang[0]));
        }
    }

    if !tags.iter().any(|&tag| is_encoding_tag(tag)) {
        if is_text_file(path) {
            tags.insert(tags::TEXT);
        } else {
            tags.insert(tags::BINARY);
        }
    }

    Ok(tags.into_iter().collect())
}

fn tags_from_filename(filename: &Path) -> Vec<&str> {
    let ext = filename.extension().and_then(|ext| ext.to_str());
    let filename = filename
        .file_name()
        .and_then(|name| name.to_str())
        .expect("Invalid filename");

    let mut result = FxHashSet::default();

    if let Some(tags) = by_filename().get(filename) {
        for tag in *tags {
            result.insert(*tag);
        }
    }
    if result.is_empty() {
        // # Allow e.g. "Dockerfile.xenial" to match "Dockerfile".
        if let Some(name) = filename.split('.').next() {
            if let Some(tags) = by_filename().get(name) {
                result.extend(&**tags);
            }
        }
    }

    if let Some(ext) = ext {
        if let Some(tags) = by_extension().get(ext) {
            result.extend(tags);
        }
    }

    result.into_iter().collect()
}

fn tags_from_interpreter(interpreter: &str) -> Vec<&'static str> {
    let Some(pos) = interpreter.rfind('/') else {
        return Vec::new();
    };
    let mut name = &interpreter[pos + 1..];
    // python3.12.3 should match python3.12.3, python3.12, python3, python
    loop {
        if let Some(tags) = by_interpreter().get(name) {
            return tags.clone();
        }
        if let Some(pos) = name.rfind('.') {
            name = &name[..pos];
        } else {
            break;
        }
    }
    vec![]
}

#[derive(thiserror::Error, Debug)]
enum ShebangError {
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

fn parse_shebang(path: &Path) -> Result<Vec<String>, ShebangError> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if !line.starts_with("#!") {
        return Err(ShebangError::NoShebang);
    }

    // Require only printable ASCII
    if line.bytes().any(|b| !(0x20..=0x7E).contains(&b)) {
        return Err(ShebangError::NonPrintableChars);
    }

    let mut tokens = shlex::split(line[2..].trim()).ok_or(ShebangError::ParseFailed)?;
    let cmd = if starts_with(&tokens, &["/usr/bin/env", "-S"]) {
        tokens.drain(0..2);
        tokens
    } else if starts_with(&tokens, &["/usr/bin/env"]) {
        tokens.drain(0..1);
        tokens
    } else {
        tokens
    };
    if cmd.is_empty() {
        return Err(ShebangError::NoCommand);
    }
    // TODO
    if cmd[0] == "nix-shell" {
        return Ok(vec![]);
    }
    Ok(cmd)
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

    let text_chars: Vec<u8> = (0..=255)
        .filter(|&x| {
            (0x20..=0x7E).contains(&x) // Printable ASCII
                || x >= 0x80 // High bit set
                || [7, 8, 9, 10, 11, 12, 13, 27].contains(&x) // Control characters
        })
        .collect();

    buffer[..bytes_read]
        .iter()
        .all(|&b| text_chars.contains(&b))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    #[cfg(unix)]
    use tempfile::tempdir;

    #[test]
    #[cfg(unix)]
    fn tags_from_path() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("source.txt");
        let dest = dir.path().join("link.txt");
        fs_err::File::create(&src).unwrap();
        std::os::unix::fs::symlink(&src, &dest).unwrap();

        let tags = super::tags_from_path(dir.path()).unwrap();
        assert_eq!(tags, vec!["directory"]);
        let tags = super::tags_from_path(&src).unwrap();
        assert_eq!(tags, vec!["plain-text", "non-executable", "file", "text"]);
        let tags = super::tags_from_path(&dest).unwrap();
        assert_eq!(tags, vec!["symlink"]);
    }

    #[test]
    fn tags_from_filename() {
        let tags = super::tags_from_filename(Path::new("test.py"));
        assert_eq!(tags, vec!["python", "text"]);

        let tags = super::tags_from_filename(Path::new("data.json"));
        assert_eq!(tags, vec!["json", "text"]);

        let tags = super::tags_from_filename(Path::new("Pipfile"));
        assert_eq!(tags, vec!["toml", "text"]);

        let tags = super::tags_from_filename(Path::new("Pipfile.lock"));
        assert_eq!(tags, vec!["json", "text"]);
    }
}
