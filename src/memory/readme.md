# Memory hierarchy modelling

This module introduces tools for modelling memory hierarchies.

## Memory fabrics

Here we think of memory modules (of any kind) as an opaque object that exposes one or more ports for communication. These ports can carry different sort of messages depending on the implementation.

We adopt the hierarchic point of view, where there is an "owner" and a "memory" parties, even when in some cases the owner itself might be a memory of another node. The goal of the interface, then, is to expose a simple control surface for the owner.

Memory modules should implement the `Memory<P>` trait, with `P` implementing then `MemoryProtocol` trait, thus modelling the fact that different memory modules might support different semantics. Memory protocols by and large consist of the sort of messages that a memory module can receive and send. Therefore, the `Memory` trait acts as the "stitches" in the memory fabric. In more complicated situations one should create custom tailored messages etc., to provide a safe and constrained environment for passing 

Then, memory modules should be modelled by `dyn Memory<P>` trait objects. This interface exposes a singe send and receive channels, which is where communication with the memory happens. The upside of this approach is that memory modules expose a uniform interface. However, the downside is that the number and nature of memory ports is obscured (e.g. `SimpleCache` has a separate read and write port, but these are not exposed to the user, rather the requests are sorted.)


## Simple
