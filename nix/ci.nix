{ pkgs, LORRI_ROOT, rust }:
let
  # Pipe a value through a few functions, left to right.
  # pipe 2 [ (v: v +1) (v: v *2) ] == 6
  # TODO upstream into nixpkgs
  pipe = val: fns: let revApply = x: f: f x; in builtins.foldl' revApply val fns;

  inherit (import ./execline.nix { inherit pkgs pipe; })
    writeExecline;

  # Write commands to script which aborts immediately if a command is not successful.
  # The status of the unsuccessful command is returned.
  allCommandsSucceed = name: commands: pipe commands [
    (pkgs.lib.concatMap (cmd: [ "if" [ cmd ] ]))
    (cmds: cmds ++ [ "true" ])
    (writeExecline name {})
  ];

  # shellcheck file
  shellcheck = file: writeExecline "lint-shellcheck" {} [
    "cd" LORRI_ROOT
    # TODO: echo is coming from context, clean out PATH before running checks
    "foreground" [ "echo" "shellchecking ${file}" ]
    "${pkgs.shellcheck}/bin/shellcheck" "--shell" "bash" file
  ];

  # the CI tests we want to run
  tests = {

    shellcheck =
      let files = [
        "nix/bogus-nixpkgs/builder.sh"
        "src/ops/direnv/envrc.bash"
      ];
      in {
        description = "shellcheck ${pkgs.lib.concatStringsSep " " files}";
        test = allCommandsSucceed "lint-shellcheck-all" (map shellcheck files);
      };

    cargo-fmt = {
      description = "cargo fmt was done";
      test = writeExecline "lint-cargo-fmt" {} [ "${rust}/bin/cargo" "fmt" "--" "--check" ];
    };

    cargo-test = {
      description = "run cargo test";
      test = writeExecline "cargo-test" {} [ "${rust}/bin/cargo" "test" ];
    };

    cargo-clippy = {
      description = "run cargo clippy";
      test = writeExecline "cargo-clippy" {} [
        "export" "RUSTFLAGS" "-D warnings"
        "${rust}/bin/cargo" "clippy"
      ];
    };

  };

in {
  inherit tests;
}
