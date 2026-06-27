/// 使用刚下载的 manifest 验证完整解析流程。
#[test]
fn test_parse_real_manifest() {
    let path = std::path::Path::new("../../data/manifests/raw/assetbundle.Chs.manifest");
    if !path.exists() {
        eprintln!("跳过: manifest 文件不存在");
        return;
    }
    let raw = std::fs::read(path).expect("读取 manifest 失败");
    let manifest = manifest::parse(&raw).expect("解析真实 manifest 失败");

    println!("assets:     {} 条", manifest.assets.len());
    println!("raw_assets: {} 条", manifest.raw_assets.len());
    println!("config:     {} 条", manifest.config.len());
    println!("load_names: {} 条", manifest.load_names.len());

    // 验证数量合理
    assert!(manifest.assets.len() > 10000, "asset 表太少");
    assert!(manifest.raw_assets.len() > 100, "raw_asset 表太少");
    assert!(manifest.config.len() > 0, "config 表不应为空");
    assert!(manifest.load_names.len() > 100, "assetname 表太少");

    // 打印前几条
    for a in manifest.assets.iter().take(3) {
        println!(
            "  asset: {} | key={} | size={} | cat={}",
            a.name, a.key, a.size, a.category
        );
    }
    for r in manifest.raw_assets.iter().take(3) {
        println!("  raw:   {} | size={} | cat={}", r.name, r.size, r.category);
    }
    for c in &manifest.config {
        println!("  config: {} = {}", c.key, c.value);
    }
    for l in manifest.load_names.iter().take(3) {
        println!("  name:  {} => {}", l.asset_name, l.name);
    }
}
