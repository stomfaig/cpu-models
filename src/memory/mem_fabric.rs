/* Core abstraction capturing what sort of interactions a memory
can handle */
pub trait MemProtocol {
    type Req;
    type Resp;
}

pub type RequestId = u8;

/* Consumer facing trait that abstracts away all the
core logic of a memory unit */
pub trait Memory<P: MemProtocol> {
    fn send(&mut self, req: P::Req) -> RequestId;
    fn recv(&mut self) -> Option<P::Resp>;
    fn tick(&mut self);
}

/* Simplest memory protocol with straight forward
reads and writes only. */
pub struct SimpleRW;

pub struct ReadReq(pub u32);
pub struct WriteReq(pub u32, pub u32);

pub enum SimpleRWReq {
    Read(ReadReq),
    Write(WriteReq),
}

impl From<ReadReq> for SimpleRWReq {
    fn from(r: ReadReq) -> Self {
        Self::Read(r)
    }
}
impl From<WriteReq> for SimpleRWReq {
    fn from(w: WriteReq) -> Self {
        Self::Write(w)
    }
}

pub struct ReadResp(pub RequestId, pub u32);
pub struct WriteResp(pub RequestId);

pub enum SimpleRWResp {
    Read(ReadResp),
    Write(WriteResp),
}

impl From<ReadResp> for SimpleRWResp {
    fn from(r: ReadResp) -> Self {
        Self::Read(r)
    }
}
impl From<WriteResp> for SimpleRWResp {
    fn from(w: WriteResp) -> Self {
        Self::Write(w)
    }
}

impl MemProtocol for SimpleRW {
    type Req = SimpleRWReq;
    type Resp = SimpleRWResp;
}
