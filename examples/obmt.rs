#![feature(box_syntax)]

use std::fmt::Debug;
use std::hash::{hash,Hash,SipHasher};
use std::collections::HashMap;
use std::thunk::Invoke;
use std::mem::replace;
//use std::sync::Arc;
use std::rc::Rc;
use std::fmt;

// "One big memo table" design

pub trait Adapton {
    fn new () -> Self ;

    // Names
    fn name_of_string (self:&mut Self, String) -> Name ;
    fn name_of_u64 (self:&mut Self, u64) -> Name ;
    fn name_pair (self: &Self, Name, Name) -> Name ;
    fn name_fork (self:&mut Self, Name) -> (Name, Name) ;

    fn ns<T,F> (self: &mut Self, Name, body:F) -> T
        where F:FnOnce(&mut Self) -> T ;

    fn put<T:Eq+Clone+Debug> (self:&mut Self, T) -> Art<T> ;

    // Mutable cells
    fn cell<T:Eq+Clone+Debug> (self:&mut Self, ArtId, T) -> MutArt<T> ;
    fn set<T:Eq+Clone+Debug> (self:&mut Self, MutArt<T>, T) ;

    fn thunk<Arg:Eq+Hash<SipHasher>+Clone+Debug,T:Eq+Clone+Debug>
        (self:&mut Self, id:ArtId,
         fn_body:Box<Invoke<(&mut Self, Arg), T> + 'static>, arg:Arg) -> Art<T> ;

    fn force<T:Eq+Clone+Debug> (self:&mut Self, Art<T>) -> T ;
}

#[derive(Hash,Debug,PartialEq,Eq)]
pub enum Lineage {
    Unknown, Structural,
    Root,
    Pair(Rc<Lineage>,Rc<Lineage>),
    ForkL(Rc<Lineage>),
    ForkR(Rc<Lineage>),
    String(String),
    U64(u64),
    Rc(Rc<Lineage>),
}

#[derive(Hash,Debug,PartialEq,Eq)]
pub enum Path {
    Empty,
    Child(Rc<Path>,Name),
}

#[derive(Hash,Debug,PartialEq,Eq)]
pub struct Name {
    hash : u64,
    lineage : Rc<Lineage>,
}

#[derive(Hash,Debug,PartialEq,Eq)]
pub struct Loc {
    hash : u64,
    name : Rc<Name>,
    path : Rc<Path>
}

#[derive(Hash,Debug,PartialEq,Eq)]
pub enum ArtId {
    None,            // Identifies an Art::Box. No dependency tracking.
    Structural(u64), // Identifies an Art::Loc.
    Nominal(Name),   // Identifies an Art::Loc.
}

#[derive(Hash,Debug,PartialEq,Eq)]
pub enum Art<T> {
    Box(Box<T>),  // No entry in table. No dependency tracking.
    Loc(Rc<Loc>), // Location in table.
}

#[derive(Hash,Debug,PartialEq,Eq)]
pub enum MutArt<T> {
    MutArt(Art<T>)
}

#[derive(Debug)]
enum Node<Res> {
    Pure(PureNode<Res>),
    Mut(MutNode<Res>),
    Compute(Box<Computer<Res>>),
}

#[derive(Debug)]
struct PureNode<T> {
    loc : Rc<Loc>,
    val : T,
}

#[derive(Debug)]
struct MutNode<T> {
    loc : Rc<Loc>,
    creators  : Vec<DemPrec>,
    dem_precs : Vec<DemPrec>,
    val : T,
}

trait Clos where Self::Comp : Computer<Self::Res> {
    type Res;
    type Comp;
    fn get_node<'x>(self:&'x mut Self) ->
        &'x mut ComputeNode<Self::Res,Self::Comp> ;
}

//#[derive(Debug)]
struct ComputeNode<Res,Comp:Computer<Res>> {
    loc : Rc<Loc>,
    creators  : Vec<DemPrec>,
    dem_precs : Vec<DemPrec>,
    dem_succs : Vec<DemSucc>,
    arg : Comp::Arg,
    res : Option<Res>,
    //body : Box<Invoke<(&'static mut A, Arg),Res> + 'static>,
    comp : Box<Comp>,
}

trait Computer<Res> {
    type Arg;
    fn compute(self:&Self, st:&mut AdaptonState, arg:Self::Arg) -> Res;
    //fn size_hint(&self) -> (usize, Option<usize>) { ... }
}

impl<Res,Comp:Computer<Res>> fmt::Debug for ComputeNode<Res,Comp> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "(ComputeNode)")
    }
}

#[derive(Debug)]
struct DemPrec {
    loc : Rc<Loc>,
}

#[derive(Debug)]
struct DemSucc {
    loc : Rc<Loc>,
    dirty : bool
}

pub struct Frame {
    path : Rc<Path>,
    name : Rc<Name>,
    succs : Vec<DemSucc>,
}

pub struct AdaptonState {
    table : HashMap<Rc<Loc>, *mut ()>,
    stack : Vec<Frame>,
}

impl fmt::Debug for AdaptonState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "(AdaptonState)")
    }
}

fn node<'x,Res,Comp:Computer<Res>> (st: &'x mut AdaptonState, loc:&Rc<Loc>) -> &'x mut Node<Res,Comp> {
    let node = st.table.get(loc) ;
    match node {
        None => panic!("dangling pointer"),
        Some(ptr) => {
            let ptr = unsafe {
                std::mem::transmute::<
                    *mut (), &mut Node<Res,Comp>
                    >(*ptr) } ;
            ptr
        }
    }
}

fn dem_precs<'x,Res:'x,Comp:Computer<Res>+'x> (st: &'x mut AdaptonState, loc: &Rc<Loc>) -> &'x mut Vec<DemPrec> {
    match *(node::<Res,Comp>(st,loc)) {
        Node::Pure(_) => {
            panic!("Node::Pure: no precs")
        },
        Node::Compute(ref mut nd) => {
            &mut nd.dem_precs
        },
        Node::Mut(ref mut nd) => {
            &mut nd.dem_precs
        }
    }
}

fn revoke_demand<'x,Res:'x,Comp:Computer<Res>+'x> (st:&mut AdaptonState, src:&Rc<Loc>, succs:&Vec<DemSucc>) {
    for succ in succs.iter() {
        let precs = dem_precs::<Res,Comp>(st, &succ.loc);
        precs.retain(|ref prec| &prec.loc != src);
    }
}

fn invoke_demand<'x,Res:'x,Comp:Computer<Res>+'x> (st:&mut AdaptonState, src:Rc<Loc>, succs:& Vec<DemSucc>) {
    for succ in succs.iter() {
        let precs = dem_precs::<Res,Comp>(st, &succ.loc);
        precs.push(DemPrec{loc:src.clone()})
    }
}

impl Adapton for AdaptonState {

    fn new () -> AdaptonState {
        let empty = Rc::new(Path::Empty);
        let root = Rc::new(Name{hash:0, lineage:Rc::new(Lineage::Root) });
        let mut stack = Vec::new();
        stack.push( Frame{path:empty, name:root, succs:Vec::new()} ) ;
        AdaptonState {
            table : HashMap::new (),
            stack : stack,
        }
    }

    fn name_of_string (self:&mut AdaptonState, sym:String) -> Name {
        let h = hash::<_,SipHasher>(&sym) ;
        let s = Lineage::String(sym) ;
        Name{ hash:h, lineage:Rc::new(s) }
    }

    fn name_of_u64 (self:&mut AdaptonState, sym:u64) -> Name {
        let h = hash::<_,SipHasher>(&sym) ;
        let s = Lineage::U64(sym) ;
        Name{ hash:h, lineage:Rc::new(s) }
    }

    fn name_pair (self: &AdaptonState, fst: Name, snd: Name) -> Name {
        let h = hash::<_,SipHasher>( &(fst.hash,snd.hash) ) ;
        let p = Lineage::Pair(fst.lineage, snd.lineage) ;
        Name{ hash:h, lineage:Rc::new(p) }
    }

    fn name_fork (self:&mut AdaptonState, nm:Name) -> (Name, Name) {
        let h1 = hash::<_,SipHasher>( &(&nm, 11111111) ) ; // TODO: make this hashing better.
        let h2 = hash::<_,SipHasher>( &(&nm, 22222222) ) ;
        ( Name{ hash:h1,
                lineage:Rc::new(Lineage::ForkL(nm.lineage.clone())) } ,
          Name{ hash:h2,
                lineage:Rc::new(Lineage::ForkR(nm.lineage)) } )
    }

    fn ns<T,F> (self: &mut Self, nm:Name, body:F) -> T where F:FnOnce(&mut Self) -> T {
        let path_body = Rc::new(Path::Child(self.stack[0].path.clone(), nm)) ;
        let path_pre = replace(&mut self.stack[0].path, path_body ) ;
        let x = body(self) ;
        let path_body = replace(&mut self.stack[0].path, path_pre) ;
        drop(path_body);
        x
    }

    fn put<T:Eq> (self:&mut AdaptonState, x:T) -> Art<T> {
        Art::Box(Box::new(x))
    }

    fn cell<T:Eq+Clone+Debug> (self:&mut AdaptonState, id:ArtId, val:T) -> MutArt<T> {
        let loc = match id {
            ArtId::None => panic!("a cell requires a unique identity"),
            ArtId::Structural(hash) => {
                Rc::new(Loc{
                    hash:hash,
                    name:Rc::new(Name{hash:hash,
                                      lineage:Rc::new(Lineage::Structural)}),
                    path:self.stack[0].path.clone()
                })
            }
            ArtId::Nominal(nm) => {
                Rc::new(Loc{
                    hash:nm.hash,
                    name:Rc::new(nm),
                    path:self.stack[0].path.clone() })
            }} ;
        let mut node = Node::Mut(MutNode{
            loc:loc.clone(),
            dem_precs:Vec::new(),
            creators:Vec::new(),
            val:val
        }) ;
        let ptr = panic!("") ;
            // unsafe { std::mem::transmute::<
            // *mut Node<T,Comp>,
            // *mut ()
            // >(&mut node) } ;
        // TODO: Check to see if the cell exists;
        // check if its content has changed.
        // dirty its precs if so.
        self.table.insert(loc.clone(), ptr) ;
        MutArt::MutArt(Art::Loc(loc))
    }

    fn set<T:Eq+Clone+Debug> (self:&mut Self, cell:MutArt<T>, val:T) {
        match cell {
            MutArt::MutArt(Art::Box(b)) => {
                panic!("cannot set pure value");
            }
            MutArt::MutArt(Art::Loc(loc)) => {
                let node = self.table.get(&loc) ;
                match node {
                    None => panic!("dangling pointer"),
                    Some(ptr) => {
                        let node : &mut MutNode<T> = unsafe {
                            let node = std::mem::transmute::<
                                *mut (), *mut Node<T,Computer<T>>
                                >(*ptr) ;
                            match *node {
                                Node::Mut(ref mut nd) => nd,
                                ref nd => panic!("impossible")
                            }} ;
                        if (node.val == val) {
                            // Nothing.
                        }
                        else {
                            node.val = val;
                            // TODO: Dirty traversal. Notify/mark demand precs.
                        }
                    }
                }
            }
        }
    }

    fn thunk<Arg:Eq+Hash<SipHasher>+Clone+Debug,T:Eq+Clone+Debug>
        (self:&mut AdaptonState,
         id:ArtId, fn_body:Box<Invoke<(&mut Self, Arg),T>+'static>, arg:Arg) -> Art<T>
    {
        match id {
            ArtId::None => {
                Art::Box(Box::new(fn_body.invoke((self,arg))))
            },
            ArtId::Structural(hash) => {
                panic!("")
            },
            ArtId::Nominal(nm) => {
                panic!("")
            }
        }
    }

    fn force<T:Eq+Clone+Debug> (self:&mut AdaptonState, art:Art<T>) -> T {
        match art {
            Art::Box(b) => *b,
            Art::Loc(loc) => {
                let node = self.table.get(&loc) ;
                match node {
                    None => panic!("dangling pointer"),
                    Some(ptr) => {
                        let node : &mut Node<T,Computer<T>> = unsafe {
                            let node = std::mem::transmute::<*mut (), *mut Node<T,Computer<T>>>(*ptr) ;
                            &mut ( *node ) } ;
                        match *node {
                            Node::Pure(ref mut nd) => {
                                nd.val.clone()
                            },
                            Node::Mut(ref mut nd) => {
                                if self.stack.is_empty() { } else {
                                    self.stack[0].succs.push(DemSucc{loc:loc.clone(),dirty:false});
                                } ;
                                nd.val.clone()
                            },
                            Node::Compute(ref mut nd) => {
                                self.stack.push ( Frame{name:loc.name.clone(),
                                                        path:loc.path.clone(),
                                                        succs:Vec::new(), } );
                                let val = panic!("TODO: run compute node body") ;
                                let mut frame = match
                                    self.stack.pop() { None => panic!(""), Some(frame) => frame } ;
                                revoke_demand( self, &nd.loc, &nd.dem_succs );
                                invoke_demand( self, nd.loc.clone(), &frame.succs );
                                nd.dem_succs = frame.succs;
                                if self.stack.is_empty() { } else {
                                    self.stack[0].succs.push(DemSucc{loc:loc.clone(),dirty:false});
                                };
                                val
                            }
                        }
                    }
                }
            }
        }
    }

}

pub fn main () {


}
