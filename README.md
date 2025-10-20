## Test application RTP

Create 3 pipelines (p1 - p3).

p1 audiotestsrc -> udpsink 239.0.0.10  
p2 udpsrc -> udpsink 239.0.0.11  
p3 udpsrc -> udpsink 239.0.0.12  
