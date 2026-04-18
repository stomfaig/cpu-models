# Model implementations for simple CPUs.

This repo contains model implementations of simple CPU models. Currently, there following implementations are available:
1. In-order 5 stage cpu
2. Simple Out-Of-Order cpu

All implementations are designed with a 2-level approach:
1. There is a main cpu implementation, that outlines the different components of the cpu, and how they interact with each other. More complicated components are only assumed to present a given trait interface.
2. For each such component, then, an implementation is provided, which can be used as a good starting point to for experiments.

(More details about the OOO cpu can be found in the readme in the `simple_ooo` directory.)

In addition, an ALU model is provided so that the implementation should allow fairly general experiments. Here one can set
- the latency of different instructions,
- the number, and capabilities of ALU pipes
which should allow most experiments, that do not focus on the ALU implementation itself. Interfacing with the general ALU happens through implementing the traits `OpaqueInstruction` and `OpaqueResult`, which allow the user of the ALU to bundle more information (e.g. ROB tags) with the instructions passed to the ALU.