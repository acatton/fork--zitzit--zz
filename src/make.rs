use super::project::{Project, Artifact};
use fasthash::metro;
use std::path::Path;
use std::path::PathBuf;
use std::collections::HashSet;
use std::process::Command;
use pbr;

pub struct Step {
    source: PathBuf,
    args:   Vec<String>
}

pub struct Make {
    artifact:   Artifact,
    steps:      Vec<Step>,
    cc:         String,
    cflags:     Vec<String>,
    lflags:     Vec<String>,
}

impl Make {
    pub fn new(mut project: Project, artifact: Artifact) -> Self {

        let mut lflags = Vec::new();
        let mut cflags = Vec::new();

        let mut cc = std::env::var("CC").unwrap_or("clang".to_string());

        if let Some(std) = project.std {
            cflags.push(format!("-std={}", std));
            if std.contains("c++") {
                cc = std::env::var("CXX").unwrap_or("clang++".to_string());
            }
        }

        if let Some(cincs) = &project.cincludes {
            for cinc in cincs {
                cflags.push("-I".into());
                cflags.push(cinc.clone());
            }
        }

        if let Some(pkgs) = &project.pkgconfig {
            for pkg in pkgs {
                let flags = Command::new("pkg-config")
                    .arg("--cflags")
                    .arg(pkg)
                    .output()
                    .expect(&format!("failed to execute pkg-config --cflags {}", pkg));

                let flags = String::from_utf8_lossy(&flags.stdout);
                let flags = flags.split_whitespace();
                for flag in flags {
                    cflags.push(flag.to_string());
                }

                let flags = Command::new("pkg-config")
                    .arg("--libs")
                    .arg(pkg)
                    .output()
                    .expect(&format!("failed to execute pkg-config --lflags {}", pkg));

                let flags = String::from_utf8_lossy(&flags.stdout);
                let flags = flags.split_whitespace();
                for flag in flags {
                    lflags.push(flag.to_string());
                }
            }
        }

        cflags.push("-fPIC".into());
        cflags.push("-I".into());
        cflags.push(".".into());
        cflags.push("-I".into());
        cflags.push("./target/include/".into());
        cflags.push("-fvisibility=hidden".to_string());


        let cobjects = std::mem::replace(&mut project.cobjects, None);

        if let Some(pcflags) = &project.cflags{
            cflags.extend(pcflags.clone());
        }
        if let Some(plflags) = &project.lflags{
            lflags.extend(plflags.clone());
        }

        let mut m = Make {
            cc,
            artifact,
            //project,
            lflags,
            cflags,
            steps: Vec::new(),
        };

        if let Some(c) = cobjects {
            for c in c {
                m.cobject(Path::new(&c));
            }
        }

        m
    }


    fn is_dirty(&self, sources: &HashSet<PathBuf>, target: &str) -> bool {
        let itarget = match std::fs::metadata(target) {
            Ok(v)  => v,
            Err(_) => return true,
        };
        let itarget = itarget.modified().expect(&format!("cannot stat {}", target));

        for source in sources {
            let isource = std::fs::metadata(source).expect(&format!("cannot stat {:?}", source));

            let isource = isource.modified().expect(&format!("cannot stat {:?}", source));

            if isource > itarget {
                return true;
            }
        }
        return false;
    }

    pub fn cobject(&mut self, inp: &Path) {

        let mut args = self.cflags.clone();
        args.push("-c".to_string());
        args.push(inp.to_string_lossy().to_string());
        args.push("-o".to_string());

        let hash = metro::hash128(args.join(" ").as_bytes());

        let outp = inp.to_string_lossy().replace(|c: char| !c.is_alphanumeric(), "_");
        let outp = format!("{}_{:x}", outp, hash);
        let outp = String::from("./target/c/") + &outp + ".o";

        args.push(outp.clone());

        let mut sources = HashSet::new();
        sources.insert(inp.into());
        if self.is_dirty(&sources, &outp) {
            self.steps.push(Step{
                source: inp.into(),
                args,
            });
        }

        self.lflags.insert(0, outp);
    }

    pub fn build(&mut self, cf: &super::emitter::CFile) {
        let mut args = self.cflags.clone();
        args.push("-Werror=implicit-function-declaration".to_string());
        args.push("-Werror=incompatible-pointer-types".to_string());
        args.push("-Werror=return-type".to_string());
        args.push("-Wpedantic".to_string());
        args.push("-Wall".to_string());
        args.push("-Wno-unused-function".to_string());
        args.push("-Wno-parentheses-equality".to_string());
        args.push("-Werror=pointer-sign".to_string());
        args.push("-Werror=int-to-pointer-cast".to_string());
        args.push("-c".to_string());
        args.push(cf.filepath.clone());
        args.push("-o".to_string());

        let hash = metro::hash128(args.join(" ").as_bytes());

        let outp = format!("./target/zz/{}_{:x}.o", cf.name, hash);
        args.push(outp.clone());

        if self.is_dirty(&cf.sources, &outp) {
            self.steps.push(Step{
                source: Path::new(&cf.filepath).into(),
                args,
            });
        }
        self.lflags.insert(0, outp);
    }


    pub fn link(mut self) {
        use rayon::prelude::*;
        use std::sync::{Arc, Mutex};

        let pb = Arc::new(Mutex::new(pbr::ProgressBar::new(self.steps.len() as u64)));
        self.steps.par_iter().for_each(|step|{
            pb.lock().unwrap().message(&format!("{} {:?} ", self.cc, step.source));

            let status = Command::new(&self.cc)
                .args(&step.args)
                .status()
                .expect("failed to execute cc");
            if !status.success() {
                error!("error compiling {:?}", step.source);
                std::process::exit(status.code().unwrap_or(3));
            }
            pb.lock().unwrap().inc();
        });

        match self.artifact.typ {
            super::project::ArtifactType::Lib => {
                self.lflags.push("-shared".into());
                self.lflags.push("-o".into());
                self.lflags.push(format!("./target/{}.so", self.artifact.name));
            },
            super::project::ArtifactType::Exe => {
                self.lflags.push("-o".into());
                self.lflags.push(format!("./target/{}", self.artifact.name));
            }
            super::project::ArtifactType::Test  => {
                self.lflags.push("-o".into());
                self.lflags.push(format!("./target/{}", self.artifact.name));
            }
            super::project::ArtifactType::Header  => {
                panic!("cannot link header yet");
            }
        }
        self.lflags.push("-fvisibility=hidden".into());

        pb.lock().unwrap().message(&format!("[WORK] ld [{:?}] {}", self.artifact.typ, self.artifact.name));
        debug!("{:?}", self.lflags);

        let status = Command::new(&self.cc)
            .args(&self.lflags)
            .status()
            .expect("failed to execute linker");
        if !status.success() {
            std::process::exit(status.code().unwrap_or(3));
        }

        pb.lock().unwrap().finish_print("done linking");
    }
}
