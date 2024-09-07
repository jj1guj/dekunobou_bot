
fn main(){
    // CMakeLists.txtが存在するディレクトリを指定します
    // プロジェクトディレクトリからの相対位置となります
    let dst = cmake::build("dekunobou/web/api/lib/dekunobou_lib/src/cpp");
    println!("cargo:rustc-link-search=native={}", dst.display());

    // staticライブラリとして他に利用するライブラリはなし
    println!("cargo:rustc-link-lib=gomp");

    // C++ソースコードの場合は必ずこれを追加すること
    println!("cargo:rustc-link-lib=dylib=stdc++");
}
