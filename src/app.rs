fn emit_evaluation(board:Board,source:Source)->AppResult<()> {
    let model=Model::new(board);let corpus=model.corpus(&source)?;let raw=full_raw(&model.original,&corpus,&model.geometry);let m=metrics(&raw,&corpus);
    let weights=load_weights(Path::new(WEIGHTS_FILE))?;let b=breakdown(&m,&weights);
    let stdout=io::stdout();let mut out=BufWriter::new(stdout.lock());
    writeln!(out,"{{\n  \"Model\": {},\n  \"Layout\": {},\n  \"Corpus\": {},\n  \"Metrics\": {{",json_quote(MODEL_VERSION),json_quote(&model.board.name),json_quote(&corpus.name))?;
    for(i,name)in METRIC_NAMES.iter().enumerate(){writeln!(out,"    {}: {:.12}{}",json_quote(name),m.v[i],if i+1==N_METRICS{""}else{","})?;}
    writeln!(out,"  }},\n  \"Simple\": {:?},\n  \"Usage\": {:?},\n  \"Off\": {:?},\n  \"Totals\": {:?},\n  \"Coverage\": {:?},\n  \"NoThumbPair\": {},\n  \"NoThumbTriple\": {},\n  \"Objective\": {{\"Penalty\": {}, \"Credit\": {}, \"Net\": {}}},\n  \"Warnings\": [",m.simple,m.usage,m.off,corpus.totals,corpus.coverage,raw.0[SRAF_DEN],raw.0[RHYTHM_DEN],b.penalty,b.bonus,b.net)?;
    for(i,line)in corpus.warnings.iter().enumerate(){writeln!(out,"    {}{}",json_quote(line),if i+1==corpus.warnings.len(){""}else{","})?;}writeln!(out,"  ]\n}}")?;out.flush()?;Ok(())
}
fn corpus_order_name(order:&str)->AppResult<(usize,Vec<&'static str>)>{
    match order{"1"=>Ok((1,vec!["letters","monograms","unigrams"])),"2"=>Ok((2,vec!["bigrams"])),
        "3"=>Ok((3,vec!["trigrams"])),"4"=>Ok((4,vec!["fourgrams","quadrigrams","quadgrams","tetragrams"])),
        "5"=>Ok((5,vec!["fivegrams"])),"skip"=>Ok((2,vec!["skipgrams"])),_=>Err("order must be 1, 2, 3, 4, 5 or skip".into())}
}
fn top_ngrams(path:&Path,order:&str,limit:usize)->AppResult<Vec<(String,f64)>> {
    let path=if path.extension().and_then(|x|x.to_str())==Some("txt"){
        let needed=order.parse::<usize>().unwrap_or(3).max(DEFAULT_ORDER);ensure_corpus(path,false,Some(needed))?
    }else{path.to_owned()};
    let(width,names)=corpus_order_name(order)?;let text=fs::read_to_string(&path)?;let text=text.strip_prefix('\u{feff}').unwrap_or(&text);let mut p=JsonParser{text,p:0};p.ws();p.expect(b'{')?;
    let mut table=None;
    loop{p.ws();if p.byte()==Some(b'}'){p.p+=1;break;}let key=p.string()?;p.ws();p.expect(b':')?;
        if names.contains(&key.as_str()){table=Some(read_frequency_table(&mut p,width,true)?);}
        else if let Ok((w,_))=corpus_order_name(match key.as_str(){"letters"|"monograms"|"unigrams"=>"1","bigrams"=>"2","trigrams"=>"3","fourgrams"|"quadgrams"|"quadrigrams"|"tetragrams"=>"4","fivegrams"=>"5","skipgrams"=>"skip",_=>"?"}){read_frequency_table(&mut p,w,false)?;}
        else{p.value(0)?;}
        p.ws();if p.byte()==Some(b'}'){p.p+=1;break;}p.expect(b',')?;
    }
    let mut rows=table.ok_or_else(||format!("{order}-gram table is absent; rebuild from raw text with --order {}",width.max(3)))?;
    rows.sort_by(|a,b|b.1.total_cmp(&a.1).then_with(||a.0.cmp(&b.0)));rows.truncate(limit);Ok(rows)
}
fn corpus_info(path:&Path)->AppResult<Vec<String>> {
    let path=if path.extension().and_then(|s|s.to_str())==Some("txt"){ensure_corpus(path,false,None)?}else{path.to_owned()};
    let source=Source::load(&path)?;let mut lines=vec![format!("{}  {}",source.name,path.display())];
    for(i,name)in["letters","bigrams","skipgrams","trigrams"].iter().enumerate(){lines.push(format!("{name:<12} {:>9} rows   {:.0} events",source.tables[i].len(),source.masses[i]));}
    let mut line=String::new();BufReader::new(File::open(&path)?).take(8192).read_line(&mut line)?;
    let prefix=line.trim().trim_end_matches(',');
    if let Ok(Json::Object(root))=parse_json(&format!("{prefix}}}")){if let Some(Json::Object(meta))=root.get("source"){
        if let Some(Json::Number(n))=meta.get("max_order"){lines.push(format!("maximum order  {n}"));}
        if let Some(Json::Array(rows))=meta.get("rows"){for(i,name)in["fourgrams","fivegrams"].iter().enumerate(){if let Some(Json::Number(n))=rows.get(i+3){if *n>0.0{lines.push(format!("{name:<12} {:>9} rows",*n as u64));}}}}
    }}
    lines.extend(source.warnings);Ok(lines)
}
fn corpus_command(args:&[String])->AppResult<()> {
    corpus_dirs()?;let action=args.first().map(String::as_str).unwrap_or("list");
    match action {
        "list"=>{for p in corpus_paths()?{println!("{:<24} {}",corpus_name(&p),p.display());}},
        "add"=>{if args.len()<3||args.len()>4{return Err("corpus add NAME INPUT.txt [CONFIG.json]".into());}
            let path=add_corpus(&args[1],Path::new(&args[2]),args.get(3).map(Path::new))?;println!("{}",path.display());},
        "build"=>{
            let mut name=None;let mut order=None;let mut jobs=None;let mut i=1;
            while i<args.len(){match args[i].as_str(){
                "--order"=>{i+=1;let n:usize=args.get(i).ok_or("--order needs a value")?.parse()?;if !(3..=5).contains(&n){return Err("--order must be 3..5".into());}order=Some(n);},
                "--jobs"=>{i+=1;let n=bounded_usize(args.get(i).ok_or("--jobs needs a value")?,16)?;jobs=Some(n);},
                value if !value.starts_with('-')&&name.is_none()=>name=Some(value.to_owned()),_=>return Err("corpus build [NAME] [--order 3|4|5] [--jobs N]".into())
            }i+=1;}
            let mut paths=Vec::new();if let Some(n)=name{let p=corpus_by_name(&n)?;if p.extension().and_then(|s|s.to_str())!=Some("txt"){return Err("raw .txt is needed to rebuild n-grams; existing JSON remains usable".into());}paths.push(p);}
            else{for e in fs::read_dir(RAW_CORPUS_DIR)?{let p=e?.path();if p.is_file()&&p.extension().and_then(|s|s.to_str())==Some("txt"){paths.push(p);}}paths.sort();}
            for p in paths{let(mut cfg,hash)=corpus_config(&corpus_config_path(&p))?;if let Some(n)=order{cfg.order=n;}if let Some(n)=jobs{cfg.jobs=n;}
                let start=Instant::now();let name=corpus_name(&p);let mut last=Instant::now()-Duration::from_secs(1);
                let output=rebuild_corpus(&p,&cfg,hash,&mut |n,total|{if last.elapsed()>Duration::from_millis(250){eprint!("\r{name} {:3.0}%",pct(n as f64,total as f64));last=Instant::now();}})?;
                eprint!("\r\x1b[2K");println!("{}  {:.2}s",output.display(),start.elapsed().as_secs_f64());
            }
        },
        "info"=>{let path=corpus_by_name(args.get(1).ok_or("corpus info NAME")?)?;for line in corpus_info(&path)?{println!("{line}");}},
        "top"=>{if args.len()<2||args.len()>4{return Err("corpus top NAME [1|2|3|4|5|skip] [COUNT]".into());}
            let path=corpus_by_name(&args[1])?;let order=args.get(2).map(String::as_str).unwrap_or("4");let n=if let Some(s)=args.get(3){bounded_usize(s,1_000_000)?}else{20};
            for(gram,count)in top_ngrams(&path,order,n)?{println!("{}\t{count}",json_quote(&gram));}},
        _=>return Err("corpus: list, add, build, info or top".into())
    }Ok(())
}
fn select_source(term:&mut Terminal,default:bool)->AppResult<Option<Source>> {
    let paths=corpus_paths()?;
    let path=if default{default_corpus_path()?}else{None};
    let path=match path{Some(p)=>Some(p),None=>{
        let names=paths.iter().map(|p|corpus_name(p)).collect::<Vec<_>>();menu(term,"",&names)?.map(|i|paths[i].clone())
    }};
    match path{Some(p)=>Ok(Some(load_source_tui(term,&p)?)),None=>Ok(None)}
}
fn raw_corpus_for(path:&Path)->Option<PathBuf>{
    if path.extension().and_then(|s|s.to_str())==Some("txt"){return Some(path.to_owned());}
    let named=Path::new(RAW_CORPUS_DIR).join(format!("{}.txt",corpus_name(path)));
    if named.is_file(){return Some(named);}
    let nearby=path.with_extension("txt");nearby.is_file().then_some(nearby)
}
fn corpus_detail_tui(term:&mut Terminal,path:&Path)->AppResult<()> {
    let raw=raw_corpus_for(path);let mut status=String::new();let mut scroll=0;
    let mut source=if let Some(raw)=&raw{
        let(_,hash)=corpus_config(&corpus_config_path(raw))?;let cache=cached_path(raw);
        if cache_current_at_least(raw,&cache,hash,3)?{Some(Source::load(&cache)?)}else{None}
    }else{Some(Source::load(path)?)};
    loop{
        let name=source.as_ref().map(|s|s.name.clone()).unwrap_or_else(||corpus_name(path));
        let mut lines=if let Some(source)=&source{corpus_info(&source.path)?}else{vec!["No current cache. Choose the largest n-gram width to store.".into()]};
        if let Some(raw)=&raw{lines.insert(0,format!("raw          {}",raw.display()));}
        else{lines.push("Rebuild unavailable: no matching raw .txt file.".into());}
        let lines=wrap_lines(&lines,term.width());let mut c=Canvas::new(term.width(),lines.len()+6);
        header(&mut c,"Corpora",&name,"",if raw.is_some(){"3 trigrams | 4 fourgrams | 5 fivegrams | q back"}else{"q back"});
        for(i,line)in lines.iter().enumerate(){c.text(0,i+3,line,if line.starts_with("Rebuild unavailable"){YELLOW}else{FG});}
        c.text(0,lines.len()+4,&short(&status,c.w),CYAN);c.h=lines.len()+6;term.present(&c,scroll)?;
        let e=term.event()?;if scroll_event(&e,&mut scroll,c.h,term.size.1){continue;}
        match e{
            Event::Escape|Event::Quit|Event::Char('q')=>return Ok(()),
            Event::Char(ch @ ('3'|'4'|'5'))=>if let Some(raw)=&raw{
                let order=ch.to_digit(10).unwrap()as usize;let(mut cfg,hash)=corpus_config(&corpus_config_path(raw))?;cfg.order=order;
                match rebuild_source_tui(term,raw,cfg,hash){
                    Ok(cache)=>match Source::load(&cache){Ok(next)=>{source=Some(next);status=format!("rebuilt through {order}-grams");},Err(e)=>status=e.to_string()},
                    Err(e)=>status=e.to_string(),
                }scroll=0;
            }else{status="raw .txt is required to rebuild".into();},
            _=>{}
        }
    }
}
fn corpus_tui(term:&mut Terminal)->AppResult<()> {
    loop{
        let paths=corpus_paths()?;let names=paths.iter().map(|p|corpus_name(p)).collect::<Vec<_>>();
        let i=match menu(term,"Corpora",&names)?{Some(i)=>i,None=>return Ok(())};corpus_detail_tui(term,&paths[i])?;if term.quitting{return Ok(());}
    }
}
fn tui_mode(term:&mut Terminal,mode:&str,board:Option<&Path>,corpus:Option<&str>)->AppResult<()> {
    let source=if let Some(name)=corpus{load_source_tui(term,&corpus_by_name(name)?)?}else{match select_source(term,true)?{Some(s)=>s,None=>return Ok(())}};
    if mode=="ranker"{return ranking(term,source);}
    loop{
        if term.quitting{return Ok(());}
        let path=if let Some(p)=board{p.to_owned()}else{match choose_path(term,LAYOUT_DIR,"dat","","")?{Some(p)=>p,None=>return Ok(())}};
        let action=crate::action_keys::Layout::load(&path).ok().filter(|l|l.extended());
        let result=if let Some(layout)=action{
            match crate::action_ui::load_corpus_tui(term,&source.path){
                Ok(Some(text))=>crate::action_ui::action_editor(term,layout,text,mode=="optimizer"),
                Ok(None)=>Ok(()),Err(e)=>Err(e),
            }
        }else{let b=load_board(&path)?;if mode=="optimizer"{optimizer(term,b,&source)}else{editor(term,b,&source)}};
        if let Err(e)=result{if !term.quitting{info_page(term,"Error",&[e.to_string()])?;}}
        if board.is_some(){return Ok(());}
    }
}
fn run()->AppResult<()> {
    let args:Vec<String>=std::env::args().skip(1).collect();
    if let Some(cmd)=args.first(){match cmd.as_str(){
        "--help"|"-h"|"help"=>{println!("layouter [editor | ranker | optimizer]\nlayouter editor|optimizer [LAYOUT.dat] [CORPUS]\nlayouter ranker [CORPUS]\nlayouter eval LAYOUT.dat [CORPUS]\nlayouter corpus list|info NAME|add NAME INPUT.txt [CONFIG.json]\nlayouter corpus build [NAME] [--order 3|4|5] [--jobs N]\nlayouter corpus top NAME [ORDER] [COUNT]\n\nDefault corpus: corpus-reddit.json. Layouts: layouts/. Raw text: corpus/raw/.");return Ok(());},
        "corpus"=>return corpus_command(&args[1..]),
        "import"=>{let mut rest=vec!["add".to_string()];rest.extend_from_slice(&args[1..]);return corpus_command(&rest);},
        "eval"=>{if args.len()<2||args.len()>3{return Err("eval LAYOUT.dat [CORPUS]".into());}
            let corpus=if let Some(name)=args.get(2){corpus_by_name(name)?}else{default_corpus_path()?.ok_or("corpus-reddit.json not found; provide a corpus path")?};
            return emit_evaluation(load_board(Path::new(&args[1]))?,Source::load(&corpus)?);
        },
        "editor"|"edit"|"optimizer"|"optimize"|"ranker"|"rank"=>{
            let mode=match cmd.as_str(){"edit"=>"editor","optimize"=>"optimizer","rank"=>"ranker",s=>s};
            if args.len()>if mode=="ranker"{2}else{3}{return Err("too many arguments".into());}
            let mut term=Terminal::open()?;
            return if mode=="ranker"{tui_mode(&mut term,mode,None,args.get(1).map(String::as_str))}else{tui_mode(&mut term,mode,args.get(1).map(Path::new),args.get(2).map(String::as_str))};
        },_=>return Err(format!("unknown mode {cmd}; use layouter --help").into())
    }}
    let mut term=Terminal::open()?;let items=vec!["Editor".into(),"Ranker".into(),"Optimizer".into(),"Corpora".into()];
    while !term.quitting{let choice=match menu(&mut term,"layouter",&items)?{Some(i)=>i,None=>break};
        let result=if choice==3{corpus_tui(&mut term)}else{tui_mode(&mut term,["editor","ranker","optimizer"][choice],None,None)};
        if let Err(e)=result{if !term.quitting{info_page(&mut term,"Error",&[e.to_string()])?;}}
    }Ok(())
}
fn main(){
    if let Some(outcome) = crate::action_ui::dispatch(&std::env::args().skip(1).collect::<Vec<_>>()) {
        if let Err(error) = outcome { eprintln!("{error}"); std::process::exit(1); }
        return;
    }
if let Err(e)=run(){eprintln!("{e}");std::process::exit(1);}}
